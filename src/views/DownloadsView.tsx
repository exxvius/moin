import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import type {
  CSSProperties,
  MouseEvent as ReactMouseEvent,
  ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { SortArrowIcon } from "../components/icons";
import { Select } from "../components/Select";
import { SmoothScroll } from "../components/SmoothScroll";
import { ContextMenu, type MenuEntry } from "../components/ContextMenu";
import { GhostGlowLayer } from "../components/GhostGlowLayer";
import { useListSelection } from "../lib/useListSelection";
import { useStore } from "../lib/store";
import {
  formatBytes,
  formatDate,
  formatDuration,
  formatEta,
  formatSpeed,
  percent,
} from "../lib/format";
import type { Task, TaskStatus } from "../lib/types";

const STATUS_LABEL: Record<TaskStatus, string> = {
  queued: "Queued",
  connecting: "Connecting",
  downloading: "Downloading",
  paused: "Paused",
  completed: "Done",
  failed: "Failed",
  canceled: "Canceled",
};

const STATUS_CLASS: Record<TaskStatus, string> = {
  queued: "dim",
  connecting: "accent",
  downloading: "accent",
  paused: "warn",
  completed: "ok",
  failed: "bad",
  canceled: "faint",
};

// The glow tone per status, mirroring the --tone assignments in CSS. Used by
// the ghost layer to color each escaping-glow rectangle.
const STATUS_TONE: Record<TaskStatus, string> = {
  queued: "var(--text-dim)",
  connecting: "var(--accent)",
  downloading: "var(--accent)",
  paused: "var(--warn)",
  completed: "var(--ok)",
  failed: "var(--bad)",
  canceled: "var(--text-faint)",
};

type FilterId = "all" | "active" | "paused" | "done" | "issues" | "archived";

const FILTERS: { id: FilterId; label: string; match: (t: Task) => boolean }[] = [
  { id: "all", label: "All", match: (t) => !t.archived },
  {
    id: "active",
    label: "Downloading",
    match: (t) =>
      !t.archived &&
      (t.status === "downloading" ||
        t.status === "connecting" ||
        t.status === "queued"),
  },
  {
    id: "paused",
    label: "Paused",
    match: (t) => !t.archived && t.status === "paused",
  },
  {
    id: "done",
    label: "Done",
    match: (t) => !t.archived && t.status === "completed",
  },
  {
    id: "issues",
    label: "Issues",
    match: (t) => !t.archived && (t.status === "failed" || t.status === "canceled"),
  },
  { id: "archived", label: "Archived", match: (t) => t.archived },
];

// Only the columns worth sorting by stay on display; the rest live in the
// expanded card. The layout is a fixed, responsive grid — it scales to fit any
// window width, no horizontal scroll or resizing.
type SortKey =
  | "name"
  | "size"
  | "progress"
  | "status"
  | "speed"
  | "eta"
  | "avgspeed"
  | "added"
  | "completed";
type SortDir = "asc" | "desc";
interface SortLevel {
  key: SortKey;
  dir: SortDir;
}

interface Column {
  id: SortKey;
  label: string;
  num?: boolean;
  /** The column's grid track (drives both the header and each row). */
  width: string;
  /** Minimum px width, used to decide what still fits when the window shrinks. */
  min: number;
  /** Higher stays longer; the lowest-priority columns drop first when narrow. */
  priority: number;
}

const COLUMNS: Column[] = [
  { id: "name", label: "Name", width: "minmax(140px, 0.85fr)", min: 140, priority: 80 },
  { id: "size", label: "Size", num: true, width: "78px", min: 78, priority: 50 },
  { id: "progress", label: "Progress", width: "minmax(150px, 1.25fr)", min: 150, priority: 70 },
  { id: "status", label: "Status", width: "100px", min: 100, priority: 60 },
  { id: "speed", label: "Speed", num: true, width: "84px", min: 84, priority: 40 },
  { id: "eta", label: "ETA", num: true, width: "68px", min: 68, priority: 30 },
  { id: "added", label: "Added", num: true, width: "108px", min: 108, priority: 20 },
  { id: "completed", label: "Completed", num: true, width: "108px", min: 108, priority: 10 },
];

// Archived downloads can't be resumed, so their live columns (progress, status,
// speed, eta) are meaningless — show just the historical facts instead.
const ARCHIVED_COLUMNS: Column[] = [
  { id: "name", label: "Name", width: "minmax(140px, 1fr)", min: 140, priority: 80 },
  { id: "size", label: "Size", num: true, width: "92px", min: 92, priority: 50 },
  { id: "avgspeed", label: "Avg speed", num: true, width: "104px", min: 104, priority: 40 },
  { id: "added", label: "Added", num: true, width: "130px", min: 130, priority: 20 },
  { id: "completed", label: "Completed", num: true, width: "130px", min: 130, priority: 10 },
];

// Drop the lowest-priority columns until the rest fit the available width. Name
// (highest priority) always survives.
function fitColumns(all: Column[], avail: number): Column[] {
  const PAD = 32; // .dl-grid horizontal padding (2 × space-4)
  const GAP = 12; // space-3 between columns
  const BUFFER = 8; // a little slack so nothing sits flush against the edge
  const fits = (cs: Column[]) =>
    cs.reduce((s, c) => s + c.min, 0) +
      GAP * Math.max(0, cs.length - 1) +
      PAD +
      BUFFER <=
    avail;
  const cols = [...all];
  while (cols.length > 1 && !fits(cols)) {
    let lo = 0;
    for (let i = 1; i < cols.length; i++)
      if (cols[i].priority < cols[lo].priority) lo = i;
    cols.splice(lo, 1);
  }
  return cols;
}

const STATUS_RANK: Record<TaskStatus, number> = {
  downloading: 0,
  connecting: 1,
  queued: 2,
  paused: 3,
  completed: 4,
  failed: 5,
  canceled: 6,
};

// The live bar shimmer/glow cycle (must match `bar-flow`/`bar-pulse` in CSS).
const BAR_CYCLE_MS = 1800;

// Sorting by a live value buckets it, so rows only reorder once the value
// meaningfully changes — near-tied downloads keep a stable order instead of
// constantly trading places on every tick.
const PROGRESS_STEP = 0.01; // 1% of progress
const SPEED_STEP = 100_000; // 0.1 MB/s
const ETA_STEP = 5; // seconds

function fracOf(t: Task): number {
  if (t.total && t.total > 0) return t.received / t.total;
  return t.status === "completed" ? 1 : 0;
}

function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

interface Stats {
  totalDownloaded: number;
  avgSpeed: number;
  filesDone: number;
  successRate: number | null;
  timeMs: number;
}

/** All-time stats across every download (including archived). */
function computeStats(tasks: Task[]): Stats {
  let totalDownloaded = 0;
  let speedBytes = 0;
  let speedMs = 0;
  let filesDone = 0;
  let failed = 0;
  let timeMs = 0;
  for (const t of tasks) {
    totalDownloaded += t.received;
    timeMs += t.active_ms;
    if (t.active_ms > 0) {
      speedBytes += t.received;
      speedMs += t.active_ms;
    }
    if (t.status === "completed") filesDone++;
    else if (t.status === "failed") failed++;
  }
  const total = filesDone + failed;
  return {
    totalDownloaded,
    avgSpeed: speedMs > 0 ? speedBytes / (speedMs / 1000) : 0,
    filesDone,
    successRate: total > 0 ? (filesDone / total) * 100 : null,
    timeMs,
  };
}

const SORT_KEY = "moin-dl-sort";
const SORT_KEYS: SortKey[] = [
  "name",
  "size",
  "progress",
  "status",
  "speed",
  "eta",
  "avgspeed",
  "added",
];

function avgSpeedOf(t: Task): number {
  return t.active_ms > 0 ? t.received / (t.active_ms / 1000) : 0;
}
function loadSort(): SortLevel[] {
  try {
    const v = JSON.parse(localStorage.getItem(SORT_KEY) ?? "[]");
    if (Array.isArray(v)) {
      return v.filter(
        (l): l is SortLevel =>
          l &&
          SORT_KEYS.includes(l.key) &&
          (l.dir === "asc" || l.dir === "desc"),
      );
    }
  } catch {
    // ignore malformed storage
  }
  return [];
}

interface DownloadsViewProps {
  /** Slide rows when the sort reorders (off = they jump into place). */
  animateReorder: boolean;
}

export function DownloadsView({ animateReorder }: DownloadsViewProps) {
  const store = useStore();
  const [filter, setFilter] = useState<FilterId>("all");
  const [query, setQuery] = useState("");
  const [sortStack, setSortStack] = useState<SortLevel[]>(loadSort);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  // Only one card is open at a time; opening another collapses the previous.
  const [expanded, setExpanded] = useState<string | null>(null);
  // When true, the menu's "Remove" has expanded into remove/delete choices.
  const [confirmRemove, setConfirmRemove] = useState(false);
  // Error message to surface in a popup (e.g. a file delete that failed).
  const [error, setError] = useState<string | null>(null);
  // Live width of the list, so columns can drop out when the window is narrow.
  const [listWidth, setListWidth] = useState(0);

  useEffect(() => {
    localStorage.setItem(SORT_KEY, JSON.stringify(sortStack));
  }, [sortStack]);

  const q = query.trim().toLowerCase();
  const active = FILTERS.find((f) => f.id === filter)!;
  const rows = store.all.filter(
    (t) => active.match(t) && (q === "" || t.filename.toLowerCase().includes(q)),
  );
  const sorted = sortRows(rows, sortStack, store.speeds);

  const counts: Record<FilterId, number> = {
    all: 0,
    active: 0,
    paused: 0,
    done: 0,
    issues: 0,
    archived: 0,
  };
  for (const t of store.all) for (const f of FILTERS) if (f.match(t)) counts[f.id]++;

  const stats = computeStats(store.all);
  const totalSpeed = store.all.reduce(
    (sum, t) => sum + (t.status === "downloading" ? store.speeds[t.id] ?? 0 : 0),
    0,
  );
  const downloadingCount = store.all.filter(
    (t) => t.status === "downloading",
  ).length;

  const isArchived = filter === "archived";
  const allColumns = isArchived ? ARCHIVED_COLUMNS : COLUMNS;
  const columns =
    listWidth > 0 ? fitColumns(allColumns, listWidth) : allColumns;
  const gridTemplate = columns.map((c) => c.width).join(" ");
  const primary = sortStack[0];
  const sortBy = (col: SortKey) => {
    setSortStack((prev) => {
      if (prev[0]?.key === col) {
        const [head, ...rest] = prev;
        return [{ key: col, dir: head.dir === "asc" ? "desc" : "asc" }, ...rest];
      }
      const without = prev.filter((l) => l.key !== col);
      return [{ key: col, dir: col === "name" ? "asc" : "desc" }, ...without];
    });
  };

  // FLIP: when the sorted order changes, animate each card sliding from its old
  // position to its new one (e.g. one download passing another by progress).
  const listRef = useRef<HTMLDivElement>(null);
  const prevTops = useRef<Map<string, number>>(new Map());
  const prevOrder = useRef<string[]>([]);
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const cards = Array.from(list.querySelectorAll<HTMLElement>(".dl-card"));
    const order = cards.map((c) => c.dataset.id ?? "");
    const reordered =
      order.length === prevOrder.current.length &&
      order.some((id, i) => prevOrder.current[i] !== id);

    for (const el of cards) {
      const id = el.dataset.id ?? "";
      const top = el.offsetTop;
      const prev = prevTops.current.get(id);
      if (animateReorder && reordered && prev != null && prev !== top) {
        el.style.transition = "none";
        el.style.transform = `translateY(${prev - top}px)`;
        requestAnimationFrame(() => {
          el.style.transition = "transform 320ms cubic-bezier(0.2, 0.9, 0.3, 1)";
          el.style.transform = "";
        });
      }
      prevTops.current.set(id, top);
    }
    const ids = new Set(order);
    for (const id of Array.from(prevTops.current.keys())) {
      if (!ids.has(id)) prevTops.current.delete(id);
    }
    prevOrder.current = order;
  });

  // Track the list's width so columns can drop out when there's no room.
  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const measure = () => setListWidth(el.clientWidth);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [rows.length > 0]);

  const toggle = (id: string) =>
    setExpanded((prev) => (prev === id ? null : id));

  const orderedIds = sorted.map((t) => t.id);
  const sel = useListSelection({
    containerRef: listRef,
    orderedIds,
    enabled: rows.length > 0,
    onActivate: toggle,
  });

  // The ghost layer draws each highlighted row's glow outside the clipped list
  // so it can spill past the sides. It tracks the rows on its own.
  const taskById = new Map(store.all.map((t) => [t.id, t]));
  const toneOf = (id: string): string | null => {
    const t = taskById.get(id);
    if (!t) return null;
    return t.archived ? "var(--accent)" : STATUS_TONE[t.status];
  };

  // Ctrl/Cmd+A selects everything visible; Esc clears (unless a menu owns Esc).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
        e.preventDefault();
        sel.selectAll();
      } else if (e.key === "Escape" && !menu) {
        sel.clear();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [sel.selectAll, sel.clear, menu]);

  // Right-click keeps an existing multi-selection; otherwise it targets the row
  // under the cursor. The menu's actions then apply to the whole selection.
  const openMenu = (e: ReactMouseEvent, task: Task) => {
    e.preventDefault();
    setConfirmRemove(false);
    sel.ensure(task.id);
    setMenu({ x: e.clientX, y: e.clientY });
  };

  const closeMenu = () => {
    setMenu(null);
    setConfirmRemove(false);
  };

  const copyLinks = (tasks: Task[]): MenuEntry => ({
    label: tasks.length > 1 ? `Copy ${tasks.length} links` : "Copy link",
    onClick: () =>
      navigator.clipboard
        ?.writeText(tasks.map((t) => t.url).join("\n"))
        .catch(() => {}),
  });

  // Delete finished files from disk and drop the rest (partials) from the list,
  // surfacing the first failure if a file couldn't be removed.
  const deleteSelection = (tasks: Task[]) => {
    const jobs = tasks.map((t) =>
      t.status === "completed" ? store.delete(t.id) : store.remove(t.id),
    );
    Promise.allSettled(jobs).then((results) => {
      const failed = results.find((r) => r.status === "rejected");
      if (failed?.status === "rejected") setError(errorText(failed.reason));
    });
  };

  const menuItems = (tasks: Task[]): MenuEntry[] =>
    tasks.length <= 1 ? singleMenu(tasks[0]) : multiMenu(tasks);

  // Single row: the original, nicely-worded per-item menu.
  const singleMenu = (task: Task): MenuEntry[] => {
    if (task.archived) {
      const items: MenuEntry[] = [
        { label: "Download again", onClick: () => store.retry(task.id) },
      ];
      if (task.status === "completed") {
        items.push({
          label: "Show in folder",
          onClick: () => revealItemInDir(task.dest).catch(() => {}),
        });
      }
      items.push(copyLinks([task]), { separator: true });
      items.push({
        label: "Delete from history",
        danger: true,
        onClick: () => store.forget(task.id),
      });
      return items;
    }

    const items: MenuEntry[] = [];
    if (task.status === "paused") {
      items.push({ label: "Resume download", onClick: () => store.resume(task.id) });
    } else if (task.status === "failed" || task.status === "canceled") {
      items.push({ label: "Try again", onClick: () => store.resume(task.id) });
    } else if (task.status !== "completed") {
      items.push({ label: "Pause download", onClick: () => store.pause(task.id) });
    }
    if (task.status === "completed") {
      items.push({
        label: "Show in folder",
        onClick: () => revealItemInDir(task.dest).catch(() => {}),
      });
    }
    items.push(copyLinks([task]), { separator: true });

    const active =
      task.status !== "completed" &&
      task.status !== "failed" &&
      task.status !== "canceled";
    if (active) {
      items.push({
        label: "Cancel download",
        danger: true,
        onClick: () => store.cancel(task.id),
      });
    }

    if (task.status === "completed") {
      // A finished file exists, so let the user choose to keep or delete it.
      if (!confirmRemove) {
        items.push({
          label: "Remove…",
          danger: true,
          keepOpen: true,
          onClick: () => setConfirmRemove(true),
        });
      } else {
        items.push({
          label: "Remove from list (keep file)",
          onClick: () => store.remove(task.id),
        });
        items.push({
          label: "Delete file from disk",
          danger: true,
          onClick: () =>
            store.delete(task.id).catch((e) => setError(errorText(e))),
        });
      }
    } else {
      // Incomplete/failed: nothing worth keeping — one Remove wipes the partial.
      items.push({
        label: "Remove from list",
        danger: true,
        onClick: () => store.remove(task.id),
      });
    }
    return items;
  };

  // Several rows: one action per verb, applied to whichever rows it fits.
  const multiMenu = (tasks: Task[]): MenuEntry[] => {
    const ids = tasks.map((t) => t.id);
    const runAll = (targets: string[], fn: (id: string) => Promise<unknown>) =>
      Promise.allSettled(targets.map(fn));

    if (tasks.every((t) => t.archived)) {
      return [
        {
          label: `Download again (${tasks.length})`,
          onClick: () => runAll(ids, store.retry),
        },
        copyLinks(tasks),
        { separator: true },
        {
          label: `Delete ${tasks.length} from history`,
          danger: true,
          onClick: () => runAll(ids, store.forget),
        },
      ];
    }

    const isActive = (t: Task) =>
      t.status !== "completed" &&
      t.status !== "failed" &&
      t.status !== "canceled";
    const resumable = tasks.filter(
      (t) =>
        t.status === "paused" ||
        t.status === "failed" ||
        t.status === "canceled",
    );
    const pausable = tasks.filter((t) => isActive(t) && t.status !== "paused");
    const cancelable = tasks.filter(isActive);
    const completed = tasks.filter((t) => t.status === "completed");

    const items: MenuEntry[] = [];
    if (resumable.length) {
      items.push({
        label: `Resume ${resumable.length}`,
        onClick: () => runAll(resumable.map((t) => t.id), store.resume),
      });
    }
    if (pausable.length) {
      items.push({
        label: `Pause ${pausable.length}`,
        onClick: () => runAll(pausable.map((t) => t.id), store.pause),
      });
    }
    items.push(copyLinks(tasks), { separator: true });
    if (cancelable.length) {
      items.push({
        label: `Cancel ${cancelable.length}`,
        danger: true,
        onClick: () => runAll(cancelable.map((t) => t.id), store.cancel),
      });
    }

    if (completed.length) {
      // Some rows have a finished file worth keeping — offer the same choice.
      if (!confirmRemove) {
        items.push({
          label: `Remove ${tasks.length}…`,
          danger: true,
          keepOpen: true,
          onClick: () => setConfirmRemove(true),
        });
      } else {
        items.push({
          label: `Remove ${tasks.length} from list (keep files)`,
          onClick: () => runAll(ids, store.remove),
        });
        items.push({
          label: `Delete ${completed.length} file${
            completed.length > 1 ? "s" : ""
          } from disk`,
          danger: true,
          onClick: () => deleteSelection(tasks),
        });
      }
    } else {
      items.push({
        label: `Remove ${tasks.length} from list`,
        danger: true,
        onClick: () => runAll(ids, store.remove),
      });
    }
    return items;
  };

  const menuTasks = menu ? sorted.filter((t) => sel.selected.has(t.id)) : [];

  return (
    <div className="view downloads">
      <div className="view-head">
        <h2>Downloads</h2>
        <p>Everything moin is working on.</p>
      </div>

      <div className="card stat-card">
        <div className="stat-hero">
          <span className="stat-hero-num">
            {formatBytes(stats.totalDownloaded)}
          </span>
          <span className="stat-hero-label">downloaded all-time</span>
        </div>
        <div className="stat-row">
          <div className="stat">
            <div className="stat-num">{formatSpeed(stats.avgSpeed)}</div>
            <div className="stat-label">Avg speed</div>
          </div>
          <div className="stat">
            <div className="stat-num">{stats.filesDone}</div>
            <div className="stat-label">Files done</div>
          </div>
          <div className="stat">
            <div className="stat-num">
              {stats.successRate != null
                ? `${Math.round(stats.successRate)}%`
                : "—"}
            </div>
            <div className="stat-label">Success</div>
          </div>
          <div className="stat">
            <div className="stat-num">{formatDuration(stats.timeMs)}</div>
            <div className="stat-label">Time spent</div>
          </div>
        </div>
      </div>

      <div className="card dl-panel">
        <div className="dl-toolbar">
          <div className="filter-row">
            <Select
              value={filter}
              ariaLabel="Filter downloads"
              onChange={(v) => setFilter(v as FilterId)}
              options={FILTERS.map((f) => ({
                value: f.id,
                label: (
                  <span className="opt">
                    {f.label}
                    <span className="opt-count">{counts[f.id]}</span>
                  </span>
                ),
              }))}
            />
          </div>
          <input
            className="dl-search selectable"
            type="text"
            placeholder="Filter by name…"
            value={query}
            spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>

        {rows.length === 0 ? (
          <div className="dl-empty">
            <div className="card-title">
              {store.all.length === 0 ? "Nothing here yet" : "No matches"}
            </div>
            <p className="dim">
              {store.all.length === 0
                ? "Add a link and it'll show up here."
                : "Try a different filter or search."}
            </p>
          </div>
        ) : (
          <SmoothScroll
            ref={listRef}
            className="dl-scroll"
            behind={
              <GhostGlowLayer
                viewportRef={listRef}
                selectedIds={sel.selected}
                toneOf={toneOf}
              />
            }
            header={
              <div
                className="dl-head dl-grid"
                style={{ gridTemplateColumns: gridTemplate }}
              >
                {columns.map((c) => (
                  <button
                    key={c.id}
                    className={`sort-th${c.num ? " num" : ""}${
                      primary?.key === c.id ? " on" : ""
                    }`}
                    onClick={() => sortBy(c.id)}
                  >
                    <span className="th-label">{c.label}</span>
                    {primary?.key === c.id && (
                      <span
                        className={`sort-ind${
                          primary.dir === "asc" ? " asc" : ""
                        }`}
                      >
                        <SortArrowIcon size={13} />
                      </span>
                    )}
                  </button>
                ))}
              </div>
            }
          >
            {sorted.map((t) => (
              <Card
                key={t.id}
                task={t}
                speed={store.speeds[t.id] ?? 0}
                expanded={expanded === t.id}
                selected={sel.isSelected(t.id)}
                archived={isArchived}
                columns={columns}
                gridTemplate={gridTemplate}
                onContext={openMenu}
              />
            ))}
          </SmoothScroll>
        )}
      </div>

      <div className="dl-statusbar">
        <span>{counts.all} in list</span>
        {downloadingCount > 0 && <span>{downloadingCount} downloading</span>}
        <span className="grow" />
        {sel.selected.size > 0 && (
          <span className="sel-count">{sel.selected.size} selected</span>
        )}
        <span className="num">Total {formatSpeed(totalSpeed)}</span>
      </div>

      {sel.marquee &&
        createPortal(
          <div
            className="marquee"
            style={{
              left: sel.marquee.x,
              top: sel.marquee.y,
              width: sel.marquee.w,
              height: sel.marquee.h,
            }}
          />,
          document.body,
        )}

      {menu && menuTasks.length > 0 && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems(menuTasks)}
          heading={
            menuTasks.length > 1 ? `${menuTasks.length} selected` : undefined
          }
          onClose={closeMenu}
        />
      )}

      {error &&
        createPortal(
          <div className="modal-backdrop" onClick={() => setError(null)}>
            <div
              className="modal"
              role="alertdialog"
              onClick={(e) => e.stopPropagation()}
            >
              <div className="modal-title">Couldn't delete the file</div>
              <div className="modal-body">
                {error} The download is still in your list — fix the issue (e.g.
                close whatever's using the file) and try again.
              </div>
              <div className="modal-actions">
                <button className="btn-primary" onClick={() => setError(null)}>
                  OK
                </button>
              </div>
            </div>
          </div>,
          document.body,
        )}
    </div>
  );
}

/** Stable multi-level sort: compares by each level in the stack in order, so
 *  earlier sorts survive as tiebreakers under later ones. */
function sortRows(
  rows: Task[],
  stack: SortLevel[],
  speeds: Record<string, number>,
): Task[] {
  if (stack.length === 0) return rows;
  const speedOf = (t: Task) =>
    t.status === "downloading" ? speeds[t.id] ?? 0 : 0;
  const etaOf = (t: Task) => {
    const sp = speedOf(t);
    if (t.status !== "downloading" || sp <= 0 || t.total == null)
      return Number.POSITIVE_INFINITY;
    return (t.total - t.received) / sp;
  };
  const valueOf = (t: Task, key: SortKey): number | string => {
    switch (key) {
      case "name":
        return t.filename.toLowerCase();
      case "size":
        return t.total ?? 0;
      case "progress":
        return Math.round(fracOf(t) / PROGRESS_STEP);
      case "status":
        return STATUS_RANK[t.status];
      case "speed":
        return Math.round(speedOf(t) / SPEED_STEP);
      case "eta":
        return Math.round(etaOf(t) / ETA_STEP);
      case "avgspeed":
        return avgSpeedOf(t);
      case "added":
        return t.created_at;
      case "completed":
        return t.completed_at ?? 0;
    }
  };
  return [...rows].sort((a, b) => {
    for (const level of stack) {
      const va = valueOf(a, level.key);
      const vb = valueOf(b, level.key);
      if (va < vb) return level.dir === "asc" ? -1 : 1;
      if (va > vb) return level.dir === "asc" ? 1 : -1;
    }
    return 0;
  });
}

interface CardProps {
  task: Task;
  speed: number;
  expanded: boolean;
  selected: boolean;
  archived: boolean;
  columns: Column[];
  gridTemplate: string;
  onContext: (e: ReactMouseEvent, task: Task) => void;
}

function Card({
  task,
  speed,
  expanded,
  selected,
  archived,
  columns,
  gridTemplate,
  onContext,
}: CardProps) {
  const pct = percent(task.received, task.total);
  const remaining = task.total != null ? task.total - task.received : null;
  const dl = task.status === "downloading";
  // Indeterminate (sliding) bar only when there's genuinely no progress to show.
  // A queued/connecting task that already has partial progress shows a real bar.
  const indeterminate =
    pct == null &&
    (dl || task.status === "queued" || task.status === "connecting");
  const live = dl && !indeterminate;

  // Phase-align every live bar's shimmer/glow to a shared clock by fixing a
  // negative animation-delay when the bar goes live. All bars then show the same
  // point in the 1.8s cycle, so reordering rows never shows a phase jump. The
  // delay is computed once and held stable across re-renders (and the DOM move
  // that a reorder does, which doesn't restart the animation).
  const barDelay = useRef<string | null>(null);
  if (live) {
    if (barDelay.current === null)
      barDelay.current = `${-(performance.now() % BAR_CYCLE_MS)}ms`;
  } else {
    barDelay.current = null;
  }

  // One cell per column id — the visible columns (and their order) drive both
  // the header and this row, so adding a column is a one-line change up top.
  const cellFor = (id: SortKey): ReactNode => {
    switch (id) {
      case "name":
        return (
          <span className="dl-c-name" title={task.filename}>
            {task.filename}
          </span>
        );
      case "size":
        return (
          <span className="num dim">
            {formatBytes(task.total ?? task.received)}
          </span>
        );
      case "progress":
        return (
          <span className="dl-c-prog">
            <span
              className={`mini-bar${indeterminate ? " indeterminate" : ""}${
                live ? " live" : ""
              }`}
              style={
                pct != null
                  ? ({
                      "--p": pct / 100,
                      ...(live && barDelay.current
                        ? { "--bar-delay": barDelay.current }
                        : {}),
                    } as CSSProperties)
                  : undefined
              }
            >
              <i />
            </span>
            <span className="mini-pct">
              {pct != null ? `${Math.round(pct)}%` : ""}
            </span>
          </span>
        );
      case "status":
        return (
          <span className={`dl-status ${STATUS_CLASS[task.status]}`}>
            {STATUS_LABEL[task.status]}
          </span>
        );
      case "speed":
        return <span className="num dim">{dl ? formatSpeed(speed) : "—"}</span>;
      case "eta":
        return (
          <span className="num dim">
            {dl ? formatEta(remaining, speed) : "—"}
          </span>
        );
      case "avgspeed":
        return (
          <span className="num dim">
            {task.active_ms > 0 ? formatSpeed(avgSpeedOf(task)) : "—"}
          </span>
        );
      case "added":
        return <span className="num dim">{formatDate(task.created_at)}</span>;
      case "completed":
        return (
          <span className="num dim">
            {task.completed_at != null ? formatDate(task.completed_at) : "—"}
          </span>
        );
    }
  };

  return (
    <div
      className={`dl-card${expanded ? " open" : ""}${
        selected ? " selected" : ""
      }${archived ? " archived" : ""}`}
      data-id={task.id}
      data-status={task.status}
      onContextMenu={(e) => onContext(e, task)}
    >
      <div
        className="dl-summary dl-grid"
        style={{ gridTemplateColumns: gridTemplate }}
      >
        {columns.map((c) => (
          <Fragment key={c.id}>{cellFor(c.id)}</Fragment>
        ))}
      </div>

      <div className="dl-detail-wrap">
        <div className="dl-detail">
          <div className="dl-detail-grid">
            <div className="detail-item">
              <span className="dk">Downloaded</span>
              <span className="dv">
                {formatBytes(task.received)}
                {task.total != null ? ` of ${formatBytes(task.total)}` : ""}
              </span>
            </div>
            <div className="detail-item">
              <span className="dk">Added</span>
              <span className="dv">{formatDate(task.created_at)}</span>
            </div>
            <div className="detail-item">
              <span className="dk">Avg speed</span>
              <span className="dv">
                {task.active_ms > 0
                  ? formatSpeed(task.received / (task.active_ms / 1000))
                  : "—"}
              </span>
            </div>
            <div className="detail-item">
              <span className="dk">Time spent</span>
              <span className="dv">{formatDuration(task.active_ms)}</span>
            </div>
            <div className="detail-item wide">
              <span className="dk">Saved to</span>
              <span className="dv path selectable">{task.dest}</span>
            </div>
            <div className="detail-item wide">
              <span className="dk">Source</span>
              <span className="dv path selectable">{task.url}</span>
            </div>
            {task.error && (
              <div className="detail-item wide">
                <span className="dk">Error</span>
                <span className="dv err">{task.error}</span>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
