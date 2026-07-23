// Turns a torrent's flat file list (paths with "/" separators) into a folder
// tree, so the content list and the add-torrent modal can show collapsible
// folders and let a whole subfolder be (de)selected at once.

import type { TorrentFile } from "./types";

export interface TreeFileNode {
  kind: "file";
  /** Index into the original files array (what selection maps to). */
  index: number;
  name: string;
  /** Full path within the tree (for a stable edit key). */
  path: string;
}

export interface TreeFolderNode {
  kind: "folder";
  name: string;
  /** Full folder path from the torrent root — a stable key for collapse state. */
  path: string;
  children: TreeNode[];
  /** Every descendant file index (for folder-level selection + aggregates). */
  indices: number[];
}

export type TreeNode = TreeFileNode | TreeFolderNode;

/** Build the folder tree. Folders sort before files, both natural-sorted. */
export function buildFileTree(files: TorrentFile[]): TreeNode[] {
  const root: TreeFolderNode = {
    kind: "folder",
    name: "",
    path: "",
    children: [],
    indices: [],
  };

  files.forEach((f, index) => {
    const parts = f.path.split("/").filter(Boolean);
    let cur = root;
    for (let i = 0; i < parts.length - 1; i++) {
      const name = parts[i];
      const path = cur.path ? `${cur.path}/${name}` : name;
      let child = cur.children.find(
        (c): c is TreeFolderNode => c.kind === "folder" && c.name === name,
      );
      if (!child) {
        child = { kind: "folder", name, path, children: [], indices: [] };
        cur.children.push(child);
      }
      child.indices.push(index);
      cur = child;
    }
    cur.children.push({
      kind: "file",
      index,
      name: parts[parts.length - 1] ?? f.path,
      path: parts.join("/"),
    });
  });

  sortFolder(root);
  return root.children;
}

function sortFolder(folder: TreeFolderNode): void {
  folder.children.sort((a, b) => {
    if (a.kind !== b.kind) return a.kind === "folder" ? -1 : 1;
    return a.name.localeCompare(b.name, undefined, { numeric: true });
  });
  for (const c of folder.children) if (c.kind === "folder") sortFolder(c);
}

/** A flattened, in-order view of the tree honoring which folders are collapsed. */
export interface FlatRow {
  node: TreeNode;
  depth: number;
}

export function flattenTree(
  nodes: TreeNode[],
  expanded: ReadonlySet<string>,
  depth = 0,
): FlatRow[] {
  const rows: FlatRow[] = [];
  for (const node of nodes) {
    rows.push({ node, depth });
    if (node.kind === "folder" && expanded.has(node.path)) {
      rows.push(...flattenTree(node.children, expanded, depth + 1));
    }
  }
  return rows;
}

export type SortKey = "name" | "size" | "progress" | "remaining";
export type SortDir = "asc" | "desc";

/** Aggregate size + received bytes for a node (a file, or a folder's subtree). */
function aggregate(
  node: TreeNode,
  files: TorrentFile[],
): { size: number; received: number } {
  if (node.kind === "file") {
    const f = files[node.index];
    return { size: f?.size ?? 0, received: f?.received ?? 0 };
  }
  let size = 0;
  let received = 0;
  for (const i of node.indices) {
    size += files[i]?.size ?? 0;
    received += files[i]?.received ?? 0;
  }
  return { size, received };
}

/** Sort tree siblings by a column, recursively. Folders always sort before files
 *  when sorting by name; numeric columns sort purely by value. */
export function sortTree(
  nodes: TreeNode[],
  files: TorrentFile[],
  key: SortKey,
  dir: SortDir,
): TreeNode[] {
  const sign = dir === "asc" ? 1 : -1;
  const cmp = (a: TreeNode, b: TreeNode): number => {
    if (key === "name") {
      if (a.kind !== b.kind) return a.kind === "folder" ? -1 : 1;
      return a.name.localeCompare(b.name, undefined, { numeric: true }) * sign;
    }
    const av = aggregate(a, files);
    const bv = aggregate(b, files);
    let d = 0;
    if (key === "size") d = av.size - bv.size;
    else if (key === "remaining")
      d = av.size - av.received - (bv.size - bv.received);
    else d = av.received / (av.size || 1) - bv.received / (bv.size || 1);
    return d * sign;
  };
  const sorted = [...nodes].sort(cmp);
  return sorted.map((n) =>
    n.kind === "folder"
      ? { ...n, children: sortTree(n.children, files, key, dir) }
      : n,
  );
}

export type FolderState = "all" | "none" | "some";

/** Whether all / none / some of a folder's files are selected. */
export function folderState(
  files: TorrentFile[],
  indices: number[],
): FolderState {
  let selected = 0;
  for (const i of indices) if (files[i]?.selected) selected += 1;
  if (selected === 0) return "none";
  if (selected === indices.length) return "all";
  return "some";
}
