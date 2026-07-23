import { useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, MouseEvent as ReactMouseEvent, ReactNode } from "react";
import { ChevronRight, Folder, FolderOpen, File } from "lucide-react";
import { buildFileTree, flattenTree, folderState, sortTree } from "../lib/fileTree";
import type { FolderState, SortKey, SortDir, TreeNode } from "../lib/fileTree";
import type { TorrentFile } from "../lib/types";

interface Props {
  /** Files with `selected` reflecting current selection. */
  files: TorrentFile[];
  /** Set the given file indices selected/deselected (folders pass many). */
  onSet: (indices: number[], value: boolean) => void;
  /** Grid columns: `<checkbox> <name> <…meta>` — must match the header. */
  gridTemplate: string;
  /** Icon for a file leaf, by its name. */
  fileIcon?: (name: string) => typeof File;
  /** Trailing cells for a file row (size/progress/…). */
  renderFileMeta: (index: number) => ReactNode;
  /** Trailing cells for a folder row (aggregates). */
  renderFolderMeta: (indices: number[]) => ReactNode;
  /** Right-click a file row. */
  onFileContext?: (e: ReactMouseEvent, index: number) => void;
  /** Optional column sort (defaults to name ascending). */
  sortKey?: SortKey;
  sortDir?: SortDir;
  /** Expand top-level folders by default as they appear (nested stay collapsed). */
  defaultExpandTop?: boolean;
  /** Rename a node (double-click its name). Omit to disable renaming. */
  onRename?: (node: TreeNode, newName: string) => void;
}

/** A checkbox that supports the indeterminate (partial) state for folders. */
function TriCheck({
  state,
  onChange,
}: {
  state: FolderState | boolean;
  onChange: () => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  const indeterminate = state === "some";
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = indeterminate;
  }, [indeterminate]);
  return (
    <input
      ref={ref}
      type="checkbox"
      className="moin-check"
      checked={state === "all" || state === true}
      onChange={onChange}
      onClick={(e) => e.stopPropagation()}
    />
  );
}

/** Renders a torrent's files as a collapsible folder tree with folder-level
 *  (tri-state) selection. Layout is caller-driven via `gridTemplate` + the meta
 *  render props, so the same tree serves the content table and the add modal. */
export function FileTree({
  files,
  onSet,
  gridTemplate,
  fileIcon,
  renderFileMeta,
  renderFolderMeta,
  onFileContext,
  sortKey = "name",
  sortDir = "asc",
  defaultExpandTop = false,
  onRename,
}: Props) {
  // Which node (by path) is being renamed, and its draft text.
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const startEdit = (node: TreeNode) => {
    if (!onRename) return;
    setEditing(node.path);
    setDraft(node.name);
  };
  const commit = (node: TreeNode) => {
    const v = draft.trim();
    if (v && v !== node.name) onRename?.(node, v);
    setEditing(null);
  };
  const nameOrInput = (node: TreeNode) =>
    editing === node.path ? (
      <input
        className="ft-rename selectable"
        value={draft}
        autoFocus
        spellCheck={false}
        onChange={(e) => setDraft(e.target.value)}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") commit(node);
          else if (e.key === "Escape") setEditing(null);
        }}
        onBlur={() => commit(node)}
      />
    ) : (
      <span
        className="ft-name-text"
        title={node.name}
        onDoubleClick={(e) => {
          e.stopPropagation();
          startEdit(node);
        }}
      >
        {node.name}
      </span>
    );
  const tree = useMemo(
    () => sortTree(buildFileTree(files), files, sortKey, sortDir),
    [files, sortKey, sortDir],
  );
  // Track which folders are *expanded* — default none, so folders start collapsed.
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const rows = flattenTree(tree, expanded);

  // Auto-expand top-level folders as they first appear (e.g. the base folder a
  // layout creates), without re-expanding ones the user has collapsed since.
  const seenTops = useRef<Set<string>>(new Set());
  useEffect(() => {
    if (!defaultExpandTop) return;
    const tops = tree
      .filter((n) => n.kind === "folder")
      .map((n) => n.path);
    const fresh = tops.filter((t) => !seenTops.current.has(t));
    if (fresh.length) {
      for (const t of tops) seenTops.current.add(t);
      setExpanded((prev) => new Set([...prev, ...fresh]));
    }
  }, [tree, defaultExpandTop]);

  const toggle = (path: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  return (
    <div className="ft">
      {rows.map(({ node, depth }) => {
        const grid = { gridTemplateColumns: gridTemplate } as CSSProperties;
        const indent = { paddingLeft: depth * 18 } as CSSProperties;
        if (node.kind === "folder") {
          const st = folderState(files, node.indices);
          const isCollapsed = !expanded.has(node.path);
          const FolderIcon = isCollapsed ? Folder : FolderOpen;
          return (
            <div className="ft-row folder" key={`f:${node.path}`} style={grid}>
              <span className="ft-lead" style={indent}>
                <button
                  type="button"
                  className="ft-chev-btn"
                  onClick={() => toggle(node.path)}
                  aria-label={isCollapsed ? "Expand folder" : "Collapse folder"}
                >
                  <ChevronRight
                    size={14}
                    className={`ft-chev${isCollapsed ? "" : " open"}`}
                  />
                </button>
                <TriCheck
                  state={st}
                  onChange={() => onSet(node.indices, st !== "all")}
                />
                <span className="ft-folder-btn">
                  <button
                    type="button"
                    className="ft-folder-toggle"
                    onClick={() => toggle(node.path)}
                    aria-label={node.name}
                  >
                    <FolderIcon
                      size={15}
                      strokeWidth={2}
                      className="ft-folder-icon"
                    />
                  </button>
                  {nameOrInput(node)}
                </span>
              </span>
              {renderFolderMeta(node.indices)}
            </div>
          );
        }

        const f = files[node.index];
        const Icon = fileIcon?.(node.name) ?? File;
        return (
          <div
            className="ft-row"
            key={`i:${node.index}`}
            style={grid}
            onContextMenu={
              onFileContext ? (e) => onFileContext(e, node.index) : undefined
            }
          >
            <span
              className={`ft-lead file${f?.selected ? "" : " skipped"}`}
              style={indent}
            >
              <span className="ft-chev-spacer" />
              <TriCheck
                state={f?.selected ?? false}
                onChange={() => onSet([node.index], !(f?.selected ?? false))}
              />
              <Icon size={15} strokeWidth={2} className="ft-file-icon" />
              {nameOrInput(node)}
            </span>
            {renderFileMeta(node.index)}
          </div>
        );
      })}
    </div>
  );
}
