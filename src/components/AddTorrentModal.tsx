import { useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { open } from "@tauri-apps/plugin-dialog";
import { Select } from "./Select";
import { CategoryIcon } from "./CategoryIcon";
import { FileTree } from "./FileTree";
import { useStore } from "../lib/store";
import { api } from "../lib/api";
import { formatBytes } from "../lib/format";
import type { TorrentPreview } from "../lib/types";

interface Props {
  /** Magnet URI or local .torrent path to resolve and add. */
  source: string;
  onClose: () => void;
}

/** The add-a-torrent flow: resolve the source's metadata, then let the user pick
 *  which files to grab, where to save, and which category — before it queues.
 *  Adding never starts a torrent behind the user's back. */
export function AddTorrentModal({ source, onClose }: Props) {
  const store = useStore();
  const [preview, setPreview] = useState<TorrentPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Per-file selection (index-aligned with preview.files), the save folder, and
  // the category — filled in once the torrent resolves.
  const [picked, setPicked] = useState<boolean[]>([]);
  const [dir, setDir] = useState("");
  const [category, setCategory] = useState("");
  // Once the user picks a folder by hand (Browse…), stop auto-following the
  // category's folder so their choice sticks.
  const [dirTouched, setDirTouched] = useState(false);
  // Content layout, qBittorrent-style: keep the torrent's own structure
  // (Original), always nest under a torrent-named folder (subfolder), or flatten
  // files straight into the save folder (flat).
  const [layout, setLayout] = useState<"original" | "subfolder" | "flat">(
    "original",
  );
  // Editable relative file paths (index-aligned) + the base folder name — both
  // adjustable via renaming in the tree.
  const [filePaths, setFilePaths] = useState<string[]>([]);
  const [contentFolder, setContentFolder] = useState("");

  // Resolve the torrent's metadata on open (magnets reach the swarm — may pause
  // here for a moment or time out).
  useEffect(() => {
    let alive = true;
    api
      .prepareTorrent(source)
      .then((p) => {
        if (!alive) return;
        setPreview(p);
        setPicked(p.files.map((f) => f.selected));
        setFilePaths(p.files.map((f) => f.path));
        setContentFolder(p.name?.trim() || "torrent");
        setDir(p.default_dir);
        setCategory(p.suggested_category ?? "");
        setLayout("original");
      })
      .catch((e) => {
        if (alive) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      alive = false;
    };
  }, [source]);

  // Esc closes (while nothing is mid-add).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  const selectedCount = picked.filter(Boolean).length;
  const selectedSize = useMemo(() => {
    if (!preview) return 0;
    return preview.files.reduce(
      (sum, f, i) => (picked[i] ? sum + f.size : sum),
      0,
    );
  }, [preview, picked]);

  const setFiles = (indices: number[], value: boolean) => {
    const set = new Set(indices);
    setPicked((prev) => prev.map((v, i) => (set.has(i) ? value : v)));
  };
  const setAll = (value: boolean) => setPicked((prev) => prev.map(() => value));

  // Whether the chosen layout nests the content under a torrent-named folder.
  const nest =
    layout === "subfolder" ||
    (layout === "original" && (preview?.files.length ?? 0) > 1);

  // Files with `selected` mirroring the current picks + any renames, for the
  // tree. When the layout nests, prefix the base folder so the tree shows the
  // subfolder with all content beneath it, matching what lands on disk.
  const treeFiles = useMemo(() => {
    if (!preview) return [];
    const root = nest ? contentFolder.trim() || "torrent" : "";
    return preview.files.map((f, i) => ({
      ...f,
      selected: picked[i] ?? false,
      path: root ? `${root}/${filePaths[i] ?? f.path}` : filePaths[i] ?? f.path,
    }));
  }, [preview, picked, nest, contentFolder, filePaths]);

  const replaceLast = (path: string, name: string) => {
    const i = path.lastIndexOf("/");
    return i < 0 ? name : `${path.slice(0, i + 1)}${name}`;
  };
  // Rename a node in the tree: the base folder maps to the content-folder name;
  // a file or an inner folder rewrites the relative paths.
  const rename = (
    node: { kind: "file" | "folder"; path: string; index?: number },
    newName: string,
  ) => {
    if (nest && node.kind === "folder" && node.path === contentFolder) {
      setContentFolder(newName);
      return;
    }
    const rel = nest ? node.path.slice(contentFolder.length + 1) : node.path;
    if (node.kind === "file" && node.index != null) {
      setFilePaths((prev) =>
        prev.map((p, i) => (i === node.index ? replaceLast(p, newName) : p)),
      );
    } else {
      const newRel = replaceLast(rel, newName);
      setFilePaths((prev) =>
        prev.map((p) =>
          p.startsWith(`${rel}/`) ? `${newRel}${p.slice(rel.length)}` : p,
        ),
      );
    }
  };

  const pickFolder = async () => {
    const chosen = await open({
      multiple: false,
      directory: true,
      title: "Choose where to save",
      defaultPath: dir || undefined,
    });
    if (typeof chosen === "string") {
      setDir(chosen);
      setDirTouched(true);
    }
  };

  // Keep the save folder in step with the chosen category (unless the user has
  // picked a folder by hand). The backend resolves the exact folder so it matches
  // where the download would actually land.
  const changeCategory = (id: string) => {
    setCategory(id);
    if (dirTouched) return;
    api
      .categoryFolder(id || null)
      .then(setDir)
      .catch(() => {});
  };

  const add = async () => {
    if (!preview || busy || selectedCount === 0) return;
    setBusy(true);
    setError(null);
    try {
      const indices = picked
        .map((v, i) => (v ? i : -1))
        .filter((i) => i >= 0);
      await api.addTorrent(
        source,
        dir,
        category || null,
        indices,
        nest ? contentFolder.trim() || "torrent" : null,
        filePaths,
      );
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };

  return createPortal(
    <div className="modal-backdrop" onClick={busy ? undefined : onClose}>
      <div
        className="modal add-torrent-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Add a torrent"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-title">
          {preview ? preview.name || "Add torrent" : "Resolving torrent…"}
        </div>

        {!preview && !error && (
          <div className="tm-resolving">
            <span className="tm-spinner" aria-hidden />
            <p className="dim">
              Fetching the torrent's details. Magnets can take a moment.
            </p>
          </div>
        )}

        {error && <p className="dl-error">{error}</p>}

        {preview && (
          <>
            <div className="tm-files-head">
              <span className="dim">
                {selectedCount} of {preview.files.length} files ·{" "}
                {formatBytes(selectedSize)}
              </span>
              <span className="tm-file-actions">
                <button
                  type="button"
                  className="pill-btn"
                  onClick={() => setAll(true)}
                >
                  Select all
                </button>
                <button
                  type="button"
                  className="pill-btn"
                  onClick={() => setAll(false)}
                >
                  Select none
                </button>
              </span>
            </div>

            <div className="tm-files selectable">
              <FileTree
                files={treeFiles}
                onSet={setFiles}
                onRename={(node, name) =>
                  rename(
                    {
                      kind: node.kind,
                      path: node.path,
                      index: node.kind === "file" ? node.index : undefined,
                    },
                    name,
                  )
                }
                defaultExpandTop={nest}
                gridTemplate="minmax(0, 1fr) auto"
                renderFileMeta={(i) => (
                  <span className="right num dim tm-file-size">
                    {formatBytes(preview.files[i].size)}
                  </span>
                )}
                renderFolderMeta={(indices) => (
                  <span className="right num dim tm-file-size">
                    {formatBytes(
                      indices.reduce((s, i) => s + preview.files[i].size, 0),
                    )}
                  </span>
                )}
              />
            </div>

            <div className="tm-field">
              <span className="dk">Save to</span>
              <div className="tm-folder">
                <span className="tm-folder-path path selectable" title={dir}>
                  {dir}
                </span>
                <button type="button" className="dl-btn" onClick={pickFolder}>
                  Browse…
                </button>
              </div>
            </div>

            <div className="tm-field">
              <span className="dk">Content layout</span>
              <Select
                value={layout}
                ariaLabel="Content layout"
                caret
                onChange={(v) =>
                  setLayout(v as "original" | "subfolder" | "flat")
                }
                options={[
                  { value: "original", label: "Original" },
                  { value: "subfolder", label: "Create subfolder" },
                  { value: "flat", label: "Don't create subfolder" },
                ]}
              />
            </div>
          </>
        )}

        <div className="add-modal-foot">
          {preview && store.categories.length > 0 && (
            <Select
              value={category}
              ariaLabel="Category"
              caret
              onChange={changeCategory}
              options={[
                { value: "", label: "Uncategorized" },
                ...store.categories.map((c) => ({
                  value: c.id,
                  label: (
                    <span className="accent-option">
                      <CategoryIcon icon={c.icon} color={c.color} size={16} />
                      {c.name}
                    </span>
                  ),
                })),
              ]}
            />
          )}
          <div className="add-modal-actions">
            <button className="dl-btn" onClick={onClose} disabled={busy}>
              Cancel
            </button>
            <button
              className="btn-primary"
              onClick={add}
              disabled={!preview || busy || selectedCount === 0}
            >
              {busy ? "Adding…" : "Download"}
            </button>
          </div>
        </div>
      </div>
    </div>,
    document.body,
  );
}
