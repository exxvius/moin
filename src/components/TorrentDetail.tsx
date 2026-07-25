import { useEffect, useMemo, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import {
  ArrowDown,
  ArrowUp,
  Scale,
  Info,
  List,
  Radio,
  Users,
  Sprout,
  Files,
  Clock,
  File,
  FileAudio,
  FileVideo,
  FileImage,
  FileArchive,
  FileText,
  FileCode,
} from "lucide-react";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { ContextMenu, type MenuEntry } from "./ContextMenu";
import { FileTree } from "./FileTree";
import { api } from "../lib/api";
import {
  formatBytes,
  formatSpeed,
  formatDate,
  formatDuration,
  formatEta,
  formatPercent,
  percent,
} from "../lib/format";
import type { Task, TorrentDetails, TorrentFile } from "../lib/types";
import type { SortKey, SortDir } from "../lib/fileTree";

interface Props {
  task: Task;
  /** Live download speed (bytes/sec) from the card. */
  speed: number;
}

type Tab = "general" | "details" | "content" | "peers" | "trackers";

const TABS: { id: Tab; label: string; icon: typeof Info }[] = [
  { id: "general", label: "General", icon: Info },
  { id: "details", label: "Details", icon: List },
  { id: "content", label: "Content", icon: Files },
  { id: "peers", label: "Peers", icon: Users },
  { id: "trackers", label: "Trackers", icon: Radio },
];

/** Join the torrent's output folder with a file's relative path, matching the
 *  separator the dest already uses (backslash on Windows). A mixed-separator path
 *  is rejected by the Windows shell-open, so the file must be joined cleanly. */
function filePath(dest: string, rel: string): string {
  const sep = dest.includes("\\") ? "\\" : "/";
  const base = dest.replace(/[\\/]+$/, "");
  return base + sep + rel.split("/").join(sep);
}

/** A file-type icon from the extension, so the content list reads at a glance. */
function fileIcon(path: string): typeof File {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (["mp3", "flac", "wav", "aac", "ogg", "m4a", "opus"].includes(ext))
    return FileAudio;
  if (["mp4", "mkv", "avi", "mov", "webm", "wmv", "m4v", "flv"].includes(ext))
    return FileVideo;
  if (["jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "avif"].includes(ext))
    return FileImage;
  if (["zip", "rar", "7z", "tar", "gz", "iso", "bz2"].includes(ext))
    return FileArchive;
  if (["txt", "md", "nfo", "pdf", "doc", "docx", "srt"].includes(ext))
    return FileText;
  if (["js", "ts", "py", "rs", "c", "cpp", "go", "json", "html", "css"].includes(ext))
    return FileCode;
  return File;
}

/** The expanded view for a torrent: a tabbed panel with an overview, its files
 *  (pick, open, or reveal each), connected peers, and trackers. Polls live detail
 *  while it's mounted (i.e. while the card is open). */
export function TorrentDetail({ task, speed }: Props) {
  const [tab, setTab] = useState<Tab>("general");
  const [details, setDetails] = useState<TorrentDetails | null>(null);

  // Poll live detail while open. First tick is immediate.
  useEffect(() => {
    let alive = true;
    const tick = () =>
      api
        .torrentDetails(task.id)
        .then((d) => {
          if (alive) setDetails(d);
        })
        .catch(() => {});
    tick();
    const h = setInterval(tick, 1000);
    return () => {
      alive = false;
      clearInterval(h);
    };
  }, [task.id]);

  // Build the file list from the task's own paths (the torrent's native layout,
  // wrapping folder and all — same as the add-torrent modal's tree), and overlay
  // the live per-file progress + selection from the polled detail by index. The
  // backend reports paths relative to the output folder, which drops the wrapping
  // folder when a subfolder layout is used — merging by index keeps the tree
  // consistent between the two views. Fall back to the detail's own files only when
  // the task has none yet (e.g. a magnet still resolving).
  const files = useMemo(() => {
    const base = task.files ?? [];
    if (base.length === 0) return details?.files ?? [];
    const live = details?.files;
    if (!live) return base;
    return base.map((f, i) => ({
      ...f,
      received: live[i]?.received ?? f.received,
      selected: live[i]?.selected ?? f.selected,
    }));
  }, [task.files, details]);

  const applySelection = async (next: TorrentFile[]) => {
    if (details) setDetails({ ...details, files: next });
    const selected = next
      .map((f, i) => (f.selected ? i : -1))
      .filter((i) => i >= 0);
    try {
      await api.setTorrentFiles(task.id, selected);
    } catch {
      // Reconciled by the next poll if it didn't take.
    }
  };

  const setFiles = (indices: number[], value: boolean) => {
    const set = new Set(indices);
    applySelection(
      files.map((f, i) => (set.has(i) ? { ...f, selected: value } : f)),
    );
  };
  const setAll = (value: boolean) =>
    applySelection(files.map((f) => ({ ...f, selected: value })));

  return (
    <div className="td">
      <div className="td-tabs" role="tablist">
        {TABS.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            role="tab"
            aria-selected={tab === id}
            className={`td-tab${tab === id ? " active" : ""}`}
            onClick={() => setTab(id)}
          >
            <Icon size={14} strokeWidth={2.25} />
            {label}
          </button>
        ))}
      </div>

      <div className="td-panel">
        {tab === "general" && (
          <GeneralTab task={task} speed={speed} details={details} />
        )}
        {tab === "details" && <DetailsTab task={task} />}
        {tab === "content" && (
          <ContentTab
            dest={task.dest}
            files={files}
            onSet={setFiles}
            onSetAll={setAll}
          />
        )}
        {tab === "peers" && <PeersTab details={details} />}
        {tab === "trackers" && <TrackersTab details={details} />}
      </div>
    </div>
  );
}

function Tile({
  icon: Icon,
  label,
  value,
  tone,
}: {
  icon: typeof Info;
  label: string;
  value: React.ReactNode;
  tone?: "down" | "up" | "ratio" | "seeders";
}) {
  return (
    <div className={`tg-tile${tone ? ` ${tone}` : ""}`}>
      <span className="tg-tile-icon">
        <Icon size={15} strokeWidth={2.25} />
      </span>
      <span className="tg-tile-val">{value}</span>
      <span className="tg-tile-label">{label}</span>
    </div>
  );
}

function DetailRow({
  k,
  children,
  mono,
}: {
  k: string;
  children: React.ReactNode;
  mono?: boolean;
}) {
  return (
    <div className="tg-row">
      <span className="tg-row-k">{k}</span>
      <span className={`tg-row-v${mono ? " path selectable" : ""}`}>
        {children}
      </span>
    </div>
  );
}

function PieceBar({ pieces }: { pieces: number[] }) {
  if (pieces.length === 0) return null;
  return (
    <div className="tg-pieces" aria-hidden>
      {pieces.map((f, i) => (
        <span
          key={i}
          className="tg-piece"
          style={{ opacity: 0.14 + f * 0.86 }}
        />
      ))}
    </div>
  );
}

function GeneralTab({
  task,
  speed,
  details,
}: {
  task: Task;
  speed: number;
  details: TorrentDetails | null;
}) {
  const uploaded = task.uploaded ?? 0;
  const ratio = task.received > 0 ? uploaded / task.received : 0;
  const seeding = task.status === "seeding";
  const live = task.status === "downloading" || seeding;
  const remaining = task.total != null ? task.total - task.received : null;
  const pct = percent(task.received, task.total);
  const done = task.status === "completed" || seeding;
  return (
    <div className="tg">
      {details && details.pieces.length > 0 && (
        <div className="tg-pieces-block top">
          <div className="tg-pieces-head">
            <span>Pieces</span>
            <span className="dim">
              {details.availability.toFixed(3)} availability
            </span>
          </div>
          <PieceBar pieces={details.pieces} />
        </div>
      )}

      <div className="tg-progress">
        <div className="tg-progress-head">
          <span className="tg-progress-bytes">
            {formatBytes(task.received)}
            <span className="dim">
              {task.total != null ? ` of ${formatBytes(task.total)}` : ""}
            </span>
          </span>
          <span className="tg-progress-pct">
            {pct != null ? formatPercent(pct) : ""}
          </span>
        </div>
        <span className={`td-bar big${done ? " done" : ""}`} style={{ ["--p" as string]: (pct ?? 0) / 100 }}>
          <i />
        </span>
      </div>

      <div className="tg-tiles">
        <Tile
          icon={ArrowDown}
          label="Download"
          tone={live && speed > 0 ? "down" : undefined}
          value={live ? formatSpeed(speed) : "—"}
        />
        <Tile
          icon={ArrowUp}
          label="Upload"
          tone={(task.up_speed ?? 0) > 0 ? "up" : undefined}
          value={live ? formatSpeed(task.up_speed ?? 0) : "—"}
        />
        <Tile
          icon={Scale}
          label="Ratio"
          tone={ratio > 0 ? "ratio" : undefined}
          value={ratio.toFixed(2)}
        />
        <Tile
          icon={Sprout}
          label="Seeders"
          tone={(task.seeders ?? 0) > 0 ? "seeders" : undefined}
          value={task.seeders ?? 0}
        />
        <Tile icon={Users} label="Peers" value={task.peers ?? 0} />
        <Tile
          icon={Clock}
          label="ETA"
          value={live ? formatEta(remaining, speed) : "—"}
        />
      </div>
    </div>
  );
}

/** The Details tab: the textual facts about a torrent that used to sit at the
 *  bottom of General (upload/leechers/time/paths/hash/error). */
function DetailsTab({ task }: { task: Task }) {
  return (
    <div className="tg">
      <div className="tg-list">
        <DetailRow k="Uploaded">{formatBytes(task.uploaded ?? 0)}</DetailRow>
        <DetailRow k="Leechers">{task.leechers ?? 0}</DetailRow>
        <DetailRow k="Time active">{formatDuration(task.active_ms)}</DetailRow>
        <DetailRow k="Files">{task.files?.length ?? 0}</DetailRow>
        <DetailRow k="Added">{formatDate(task.created_at)}</DetailRow>
        <DetailRow k="Saved to" mono>
          {task.dest}
        </DetailRow>
        {task.info_hash && (
          <DetailRow k="Info hash" mono>
            {task.info_hash}
          </DetailRow>
        )}
        {task.error && (
          <div className="tg-row">
            <span className="tg-row-k">Error</span>
            <span className="tg-row-v err">{task.error}</span>
          </div>
        )}
      </div>
    </div>
  );
}

const CONTENT_GRID = "minmax(0, 1fr) 84px 200px 96px";

function fileBar(received: number, size: number, selected: boolean) {
  const pct = percent(received, size) ?? 0;
  const done = received >= size && size > 0;
  return (
    <span className="td-tprog">
      <span
        className={`td-bar${done ? " done" : ""}${selected ? "" : " skipped"}`}
        style={{ ["--p" as string]: pct / 100 }}
      >
        <i />
      </span>
      <span className="td-tpct num dim">{formatPercent(pct)}</span>
    </span>
  );
}

function ContentTab({
  dest,
  files,
  onSet,
  onSetAll,
}: {
  dest: string;
  files: TorrentFile[];
  onSet: (indices: number[], value: boolean) => void;
  onSetAll: (value: boolean) => void;
}) {
  const [menu, setMenu] = useState<{ x: number; y: number; index: number } | null>(
    null,
  );

  if (files.length === 0)
    return <p className="dim td-empty">No files yet.</p>;

  const openMenu = (e: ReactMouseEvent, index: number) => {
    e.preventDefault();
    // Stop the right-click bubbling to the row card, which would open the task's
    // own context menu on top of this one.
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, index });
  };

  const menuItems: MenuEntry[] = (() => {
    if (!menu) return [];
    const f = files[menu.index];
    const path = filePath(dest, f.path);
    const done = f.size > 0 && f.received >= f.size;
    const entries: MenuEntry[] = [];
    // Only a fully-downloaded file can be opened; a partial one has nothing (or
    // incomplete data) to hand to the OS.
    if (done) {
      entries.push({ label: "Open", onClick: () => openPath(path).catch(() => {}) });
    }
    entries.push({
      label: "Show in folder",
      onClick: () => revealItemInDir(path).catch(() => {}),
    });
    return entries;
  })();

  const renderFileMeta = (index: number) => {
    const f = files[index];
    const remaining = Math.max(0, f.size - f.received);
    return (
      <>
        <span className="right num dim">{formatBytes(f.size)}</span>
        {fileBar(f.received, f.size, f.selected)}
        <span className="right num dim">
          {f.selected ? formatBytes(remaining) : "—"}
        </span>
      </>
    );
  };

  const renderFolderMeta = (indices: number[]) => {
    const size = indices.reduce((s, i) => s + files[i].size, 0);
    const received = indices.reduce((s, i) => s + files[i].received, 0);
    const anySel = indices.some((i) => files[i].selected);
    const remaining = Math.max(0, size - received);
    return (
      <>
        <span className="right num dim">{formatBytes(size)}</span>
        {fileBar(received, size, anySel)}
        <span className="right num dim">
          {anySel ? formatBytes(remaining) : "—"}
        </span>
      </>
    );
  };

  const [sort, setSort] = useState<{ key: SortKey; dir: SortDir }>({
    key: "name",
    dir: "asc",
  });
  const onSort = (key: SortKey) =>
    setSort((s) =>
      s.key === key
        ? { key, dir: s.dir === "asc" ? "desc" : "asc" }
        : { key, dir: key === "name" ? "asc" : "desc" },
    );
  const Th = ({
    col,
    label,
    right,
  }: {
    col: SortKey;
    label: string;
    right?: boolean;
  }) => (
    <button
      type="button"
      className={`td-th${right ? " right" : ""}${sort.key === col ? " on" : ""}`}
      onClick={() => onSort(col)}
    >
      {label}
      {sort.key === col && (
        <span className="td-th-arrow">{sort.dir === "asc" ? "▲" : "▼"}</span>
      )}
    </button>
  );

  return (
    <div className="td-content">
      <div className="td-content-bar">
        <button type="button" className="pill-btn" onClick={() => onSetAll(true)}>
          Select all
        </button>
        <button
          type="button"
          className="pill-btn"
          onClick={() => onSetAll(false)}
        >
          Select none
        </button>
      </div>
      <div className="td-table">
        <div className="td-thead" style={{ gridTemplateColumns: CONTENT_GRID }}>
          <Th col="name" label="Name" />
          <Th col="size" label="Size" right />
          <Th col="progress" label="Progress" />
          <Th col="remaining" label="Remaining" right />
        </div>
        <FileTree
          files={files}
          onSet={onSet}
          gridTemplate={CONTENT_GRID}
          fileIcon={fileIcon}
          renderFileMeta={renderFileMeta}
          renderFolderMeta={renderFolderMeta}
          onFileContext={openMenu}
          sortKey={sort.key}
          sortDir={sort.dir}
        />
      </div>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}

function PeersTab({ details }: { details: TorrentDetails | null }) {
  const peers = details?.peers ?? [];
  if (peers.length === 0)
    return <p className="dim td-empty">No connected peers.</p>;
  return (
    <div className="td-table">
      <div className="td-thead peers">
        <span>Address</span>
        <span>State</span>
        <span className="right">Downloaded</span>
      </div>
      {peers.map((p) => (
        <div className="td-trow peers" key={p.addr}>
          <span className="selectable">{p.addr}</span>
          <span className="dim">{p.state}</span>
          <span className="right num dim">{formatBytes(p.downloaded)}</span>
        </div>
      ))}
    </div>
  );
}

function TrackersTab({ details }: { details: TorrentDetails | null }) {
  const trackers = details?.trackers ?? [];
  if (trackers.length === 0)
    return <p className="dim td-empty">No trackers (DHT only).</p>;
  return (
    <div className="td-trackers">
      {trackers.map((t) => (
        <div className="td-tracker selectable" key={t} title={t}>
          {t}
        </div>
      ))}
    </div>
  );
}
