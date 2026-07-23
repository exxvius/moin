import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import type {
  CSSProperties,
  MouseEvent as ReactMouseEvent,
  ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { ArrowDown, ArrowUp, Sprout } from "lucide-react";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { AddIcon, SortArrowIcon } from "../components/icons";
import { AddDownloadModal } from "../components/AddDownloadModal";
import { AddTorrentModal } from "../components/AddTorrentModal";
import { TorrentDetail } from "../components/TorrentDetail";
import { Select } from "../components/Select";
import { MultiSelect } from "../components/MultiSelect";
import { SmoothScroll } from "../components/SmoothScroll";
import { ContextMenu, type MenuEntry } from "../components/ContextMenu";
import { GhostGlowLayer } from "../components/GhostGlowLayer";
import { useListSelection } from "../lib/useListSelection";
import { useStore, type MoveProgress } from "../lib/store";
import { categorySwatch, findCategory } from "../lib/categories";
import { CategoryIcon } from "../components/CategoryIcon";
import type { Category } from "../lib/types";
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
  checking: "Checking",
  downloading: "Downloading",
  paused: "Paused",
  moving: "Moving",
  completed: "Done",
  seeding: "Seeding",
  failed: "Failed",
  stalled: "Stalled",
  canceled: "Canceled",
};

const STATUS_CLASS: Record<TaskStatus, string> = {
  queued: "dim",
  connecting: "accent",
  checking: "warn",
  downloading: "accent",
  paused: "warn",
  moving: "accent",
  completed: "ok",
  // Seeding reads as an ongoing transfer in the app accent (like downloading),
  // matching its progress bar — only a stopped torrent turns green (done).
  seeding: "accent",
  failed: "bad",
  stalled: "warn",
  canceled: "faint",
};

type FilterId =
  | "all"
  | "active"
  | "seeding"
  | "paused"
  | "done"
  | "issues"
  | "archived";

const FILTERS: { id: FilterId; label: string; match: (t: Task) => boolean }[] = [
  { id: "all", label: "All", match: (t) => !t.archived },
  {
    id: "active",
    label: "Downloading",
    match: (t) =>
      !t.archived &&
      (t.status === "downloading" ||
        t.status === "connecting" ||
        t.status === "queued" ||
        t.status === "moving"),
  },
  {
    id: "paused",
    label: "Paused",
    match: (t) => !t.archived && t.status === "paused",
  },
  {
    id: "seeding",
    label: "Seeding",
    match: (t) => !t.archived && t.status === "seeding",
  },
  {
    id: "done",
    label: "Done",
    match: (t) => !t.archived && t.status === "completed",
  },
  {
    id: "issues",
    label: "Issues",
    match: (t) =>
      !t.archived &&
      (t.status === "failed" ||
        t.status === "canceled" ||
        t.status === "stalled"),
  },
  { id: "archived", label: "Archived", match: (t) => t.archived },
];
const FILTER_MAP = Object.fromEntries(
  FILTERS.map((f) => [f.id, f]),
) as Record<FilterId, (typeof FILTERS)[number]>;

// Friendly names for the backend ids the engine stamps on each task.
const BACKEND_LABELS: Record<string, string> = {
  embedded: "Built-in",
  aria2: "aria2c",
};
function backendLabel(id: string): string {
  return BACKEND_LABELS[id] ?? id;
}

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
  | "completed"
  | "ratio"
  | "seeders"
  | "leechers"
  | "peers"
  | "uploaded"
  | "upspeed";
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
  { id: "added", label: "Added", num: true, width: "140px", min: 140, priority: 20 },
  { id: "completed", label: "Completed", num: true, width: "140px", min: 140, priority: 10 },
];

// Archived downloads can't be resumed, so their live columns (progress, status,
// speed, eta) are meaningless — show just the historical facts instead.
const ARCHIVED_COLUMNS: Column[] = [
  { id: "name", label: "Name", width: "minmax(140px, 1fr)", min: 140, priority: 80 },
  { id: "size", label: "Size", num: true, width: "92px", min: 92, priority: 50 },
  { id: "avgspeed", label: "Avg speed", num: true, width: "104px", min: 104, priority: 40 },
  { id: "added", label: "Added", num: true, width: "140px", min: 140, priority: 20 },
  { id: "completed", label: "Completed", num: true, width: "140px", min: 140, priority: 10 },
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
  checking: 1,
  seeding: 2,
  moving: 3,
  connecting: 4,
  queued: 5,
  paused: 6,
  stalled: 7,
  completed: 8,
  failed: 9,
  canceled: 10,
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
    // A checking torrent's `received` is pieces being verified on disk, not data
    // downloaded this run — counting it would inflate the all-time total, then snap
    // back when checking ends. Only count real downloaded bytes.
    if (t.status !== "checking") totalDownloaded += t.received;
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

// Every field the Sort-by selector offers — a superset of the visible columns,
// so a torrent can be ordered by ratio/seeders/etc. that have no column of their
// own. Clicking a column header sorts by that column and the selector follows.
const SORT_OPTIONS: { key: SortKey; label: string }[] = [
  { key: "added", label: "Date added" },
  { key: "completed", label: "Date completed" },
  { key: "name", label: "Name" },
  { key: "size", label: "Size" },
  { key: "progress", label: "Progress" },
  { key: "status", label: "Status" },
  { key: "speed", label: "Download speed" },
  { key: "eta", label: "ETA" },
  { key: "avgspeed", label: "Average speed" },
  { key: "ratio", label: "Ratio" },
  { key: "seeders", label: "Seeders" },
  { key: "leechers", label: "Leechers" },
  { key: "peers", label: "Peers" },
  { key: "uploaded", label: "Uploaded" },
  { key: "upspeed", label: "Upload speed" },
];
const SORT_KEYS: SortKey[] = SORT_OPTIONS.map((o) => o.key);

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

// The status + category filters persist too, so they survive leaving the page and
// relaunching (like the sort order already does).
const STATUS_KEY = "moin-dl-status";
const CATEGORY_KEY = "moin-dl-category";
const FILTER_IDS = FILTERS.map((f) => f.id);

function loadStatusSel(): Set<FilterId> {
  try {
    const v = JSON.parse(localStorage.getItem(STATUS_KEY) ?? "");
    if (Array.isArray(v)) {
      const valid = v.filter((x): x is FilterId => FILTER_IDS.includes(x));
      if (valid.length) return new Set(valid);
    }
  } catch {
    // ignore malformed storage
  }
  return new Set(["all"]);
}
function loadCategorySel(): Set<string> {
  try {
    const v = JSON.parse(localStorage.getItem(CATEGORY_KEY) ?? "");
    if (Array.isArray(v) && v.length && v.every((x) => typeof x === "string")) {
      return new Set(v);
    }
  } catch {
    // ignore malformed storage
  }
  return new Set([""]);
}

// Once a row changes position, it holds that slot for this long — so live-sorted
// rows (e.g. by progress) can't flip-flop past each other every tick.
const REORDER_COOLDOWN_MS = 1000;

// Reconcile the target sort order against the current display order: any row that
// moved within the cooldown stays pinned to its slot, and the rest sort freely
// into the gaps. Handles added/removed rows too. Returns the ids to display.
function reorderWithCooldown(
  targetIds: string[],
  prevOrder: string[],
  movedAt: Map<string, number>,
  now: number,
): string[] {
  const target = new Set(targetIds);
  const n = targetIds.length;
  const locked = (id: string) => {
    const t = movedAt.get(id);
    return t != null && now - t < REORDER_COOLDOWN_MS;
  };
  // Start from the previous order (so pinned rows keep their index), dropping
  // gone ids and appending newly-added ones.
  const kept = prevOrder.filter((id) => target.has(id));
  const known = new Set(kept);
  const base = kept.concat(targetIds.filter((id) => !known.has(id)));

  const result: (string | null)[] = new Array(n).fill(null);
  base.forEach((id, i) => {
    if (locked(id)) result[i] = id;
  });
  const pinned = new Set(result.filter((id): id is string => id != null));
  const free = targetIds.filter((id) => !pinned.has(id));
  let k = 0;
  for (let i = 0; i < n; i++) {
    if (result[i] == null) result[i] = free[k++];
  }
  return result as string[];
}

interface DownloadsViewProps {
  /** Slide rows when the sort reorders (off = they jump into place). */
  animateReorder: boolean;
}

export function DownloadsView({ animateReorder }: DownloadsViewProps) {
  const store = useStore();
  // Multi-select status filter. "all" is a reset: picking it clears the rest;
  // picking any specific status drops "all". Never empty (falls back to "all").
  const [statusSel, setStatusSel] = useState<Set<FilterId>>(loadStatusSel);
  // Multi-select category filter. "" = "All categories" (reset sentinel); "none"
  // = uncategorized; otherwise category ids. Same all-is-a-reset behavior.
  const [categorySel, setCategorySel] = useState<Set<string>>(loadCategorySel);
  const [query, setQuery] = useState("");
  const [sortStack, setSortStack] = useState<SortLevel[]>(loadSort);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  // Only one card is open at a time; opening another collapses the previous.
  const [expanded, setExpanded] = useState<string | null>(null);
  // When true, the menu's "Remove" has expanded into remove/delete choices.
  const [confirmRemove, setConfirmRemove] = useState(false);
  // When true, the menu is showing the category picker for "Move to category".
  const [moveOpen, setMoveOpen] = useState(false);
  // Error message to surface in a popup (e.g. a file delete that failed).
  const [error, setError] = useState<string | null>(null);
  // The add-a-download modal, opened from the "+" in the header.
  const [showAdd, setShowAdd] = useState(false);
  // Set to a magnet/.torrent source while the add-torrent modal is open.
  const [torrentSource, setTorrentSource] = useState<string | null>(null);
  // Live width of the list, so columns can drop out when the window is narrow.
  const [listWidth, setListWidth] = useState(0);

  useEffect(() => {
    localStorage.setItem(SORT_KEY, JSON.stringify(sortStack));
  }, [sortStack]);
  useEffect(() => {
    localStorage.setItem(STATUS_KEY, JSON.stringify([...statusSel]));
  }, [statusSel]);
  useEffect(() => {
    localStorage.setItem(CATEGORY_KEY, JSON.stringify([...categorySel]));
  }, [categorySel]);

  const q = query.trim().toLowerCase();

  const toggleStatus = (id: string) =>
    setStatusSel((prev) => {
      if (id === "all") return new Set<FilterId>(["all"]);
      const next = new Set(prev);
      next.delete("all");
      const fid = id as FilterId;
      if (next.has(fid)) next.delete(fid);
      else next.add(fid);
      return next.size === 0 ? new Set<FilterId>(["all"]) : next;
    });
  const toggleCategory = (id: string) =>
    setCategorySel((prev) => {
      if (id === "") return new Set<string>([""]);
      const next = new Set(prev);
      next.delete("");
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next.size === 0 ? new Set<string>([""]) : next;
    });

  const hiddenCats = new Set(
    store.categories.filter((c) => c.hidden_from_all).map((c) => c.id),
  );
  const matchesStatus = (t: Task) =>
    [...statusSel].some((id) => FILTER_MAP[id]?.match(t));
  const matchesCategory = (t: Task) => {
    // "All categories" shows everything but a hidden-from-all category's downloads
    // (uncategorized always shows). A specific pick is the union of those chosen.
    if (categorySel.has("")) {
      return t.category == null || !hiddenCats.has(t.category);
    }
    if (t.category == null) return categorySel.has("none");
    return categorySel.has(t.category);
  };
  const rows = store.all.filter(
    (t) =>
      matchesStatus(t) &&
      matchesCategory(t) &&
      (q === "" || t.filename.toLowerCase().includes(q)),
  );
  const sorted = sortRows(rows, sortStack, store.speeds);

  // Apply the reorder cooldown: rows that just moved hold their slot for a beat
  // so the list doesn't churn as live values cross. A sort-criteria change resets
  // it (the reset runs during render so the new sort applies immediately).
  const displayOrderRef = useRef<string[]>([]);
  const movedAtRef = useRef<Map<string, number>>(new Map());
  const prevSortRef = useRef(sortStack);
  if (prevSortRef.current !== sortStack) {
    prevSortRef.current = sortStack;
    displayOrderRef.current = [];
    movedAtRef.current.clear();
  }
  const displayIds = reorderWithCooldown(
    sorted.map((t) => t.id),
    displayOrderRef.current,
    movedAtRef.current,
    performance.now(),
  );
  const byId = new Map(rows.map((t) => [t.id, t]));
  const displayed = displayIds
    .map((id) => byId.get(id))
    .filter((t): t is Task => t != null);
  useLayoutEffect(() => {
    const now = performance.now();
    const movedAt = movedAtRef.current;
    const prevIdx = new Map(displayOrderRef.current.map((id, i) => [id, i]));
    displayIds.forEach((id, i) => {
      const p = prevIdx.get(id);
      if (p !== undefined && p !== i) movedAt.set(id, now);
    });
    const alive = new Set(displayIds);
    for (const id of Array.from(movedAt.keys())) {
      if (!alive.has(id)) movedAt.delete(id);
    }
    displayOrderRef.current = displayIds;
  });

  const counts: Record<FilterId, number> = {
    all: 0,
    active: 0,
    seeding: 0,
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

  // Show the archive-only columns only when Archived is the single active filter;
  // mixed with others, the normal live columns stay.
  const isArchived = statusSel.size === 1 && statusSel.has("archived");
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
  // Clear all sorting (back to the natural order), and flip the primary key's
  // direction — both for the Sort-by control, whose field can have no column.
  const clearSort = () => setSortStack([]);
  const toggleSortDir = () =>
    setSortStack((prev) =>
      prev.length === 0
        ? prev
        : [
            { key: prev[0].key, dir: prev[0].dir === "asc" ? "desc" : "asc" },
            ...prev.slice(1),
          ],
    );

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
          el.style.transition = "transform 240ms cubic-bezier(0.2, 0.9, 0.3, 1)";
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

  const orderedIds = displayIds;
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
    // The escaping border glow follows the category's effects color (falling back
    // to its main color); uncategorized rows glow in the theme accent. The
    // background tint uses the main color separately (see the card style).
    const cat = findCategory(store.categories, t.category);
    const glow = cat?.effects_color || cat?.color;
    if (glow) return categorySwatch(glow);
    return "var(--accent)";
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
    setMoveOpen(false);
    sel.ensure(task.id);
    setMenu({ x: e.clientX, y: e.clientY });
  };

  const closeMenu = () => {
    setMenu(null);
    setConfirmRemove(false);
    setMoveOpen(false);
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

  // The category picker the "Move to category…" entry expands into — each row
  // carries the category's icon and its color, matching the filter dropdown.
  const moveCategoryList = (tasks: Task[]): MenuEntry[] => {
    const ids = tasks.map((t) => t.id);
    const move = (category: string | null) =>
      store.moveToCategory(ids, category).catch((e) => setError(errorText(e)));
    const items: MenuEntry[] = store.categories.map((c) => ({
      label: (
        <span className="accent-option">
          <CategoryIcon
            icon={c.icon}
            color={c.color}
            iconColor={c.icon_color}
            size={15}
          />
          <span style={c.color ? { color: categorySwatch(c.color) } : undefined}>
            {c.name}
          </span>
        </span>
      ),
      onClick: () => move(c.id),
    }));
    items.push({ label: "Uncategorized", onClick: () => move(null) });
    return items;
  };

  // The entry that opens the category picker (only when categories exist).
  const moveEntry = (): MenuEntry | null =>
    store.categories.length === 0
      ? null
      : {
          label: "Move to category…",
          keepOpen: true,
          onClick: () => {
            setConfirmRemove(false);
            setMoveOpen(true);
          },
        };

  const menuItems = (tasks: Task[]): MenuEntry[] => {
    if (moveOpen) return moveCategoryList(tasks);
    return tasks.length <= 1 ? singleMenu(tasks[0]) : multiMenu(tasks);
  };

  // Single row: the original, nicely-worded per-item menu.
  const singleMenu = (task: Task): MenuEntry[] => {
    if (task.archived) {
      const items: MenuEntry[] = [
        { label: "Download again", onClick: () => store.retry(task.id) },
      ];
      if (task.status === "completed") {
        items.push({
          label: task.kind === "torrent" ? "Open folder" : "Open",
          onClick: () => openPath(task.dest).catch(() => {}),
        });
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

    // Mid-relocation: actions are held until the move finishes, so keep it simple.
    if (task.status === "moving") {
      return [copyLinks([task])];
    }

    const items: MenuEntry[] = [];
    if (task.status === "paused") {
      items.push({ label: "Resume download", onClick: () => store.resume(task.id) });
    } else if (
      task.status === "failed" ||
      task.status === "canceled" ||
      task.status === "stalled"
    ) {
      items.push({ label: "Try again", onClick: () => store.resume(task.id) });
    } else if (task.status === "seeding") {
      // A seeding torrent is done downloading; pausing it stops the upload.
      items.push({ label: "Stop seeding", onClick: () => store.pause(task.id) });
    } else if (task.status !== "completed") {
      items.push({ label: "Pause download", onClick: () => store.pause(task.id) });
    }
    // A finished torrent that stopped seeding can be put back to seeding, past any
    // ratio/time limit (it keeps going until stopped by hand).
    if (task.status === "completed" && task.kind === "torrent") {
      items.push({
        label: "Start seeding",
        onClick: () => store.startSeeding(task.id),
      });
    }
    if (task.status === "completed" || task.status === "seeding") {
      items.push({
        label: task.kind === "torrent" ? "Open folder" : "Open",
        onClick: () => openPath(task.dest).catch(() => {}),
      });
      items.push({
        label: "Show in folder",
        onClick: () => revealItemInDir(task.dest).catch(() => {}),
      });
    }
    const mv = moveEntry();
    if (mv) items.push(mv);
    items.push(copyLinks([task]), { separator: true });

    const active =
      task.status !== "completed" &&
      task.status !== "seeding" &&
      task.status !== "failed" &&
      task.status !== "canceled" &&
      task.status !== "stalled";
    if (active) {
      items.push({
        label: "Cancel download",
        danger: true,
        onClick: () => store.cancel(task.id),
      });
    }

    // A finished download, or *any* torrent, has real files on disk (a torrent's
    // partial data is genuine content) — let the user choose keep vs delete. Only
    // an incomplete HTTP download has a throwaway `.part` that one Remove wipes.
    const isTorrent = task.kind === "torrent";
    const noun = isTorrent ? "files" : "file";
    if (task.status === "completed" || isTorrent) {
      if (!confirmRemove) {
        items.push({
          label: "Remove…",
          danger: true,
          keepOpen: true,
          onClick: () => setConfirmRemove(true),
        });
      } else {
        items.push({
          label: `Remove from list (keep ${noun})`,
          onClick: () => store.remove(task.id),
        });
        items.push({
          label: `Delete ${noun} from disk`,
          danger: true,
          onClick: () =>
            store.delete(task.id).catch((e) => setError(errorText(e))),
        });
      }
    } else {
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
      t.status !== "canceled" &&
      t.status !== "stalled" &&
      t.status !== "moving";
    const resumable = tasks.filter(
      (t) =>
        t.status === "paused" ||
        t.status === "failed" ||
        t.status === "canceled" ||
        t.status === "stalled",
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
    const mv = moveEntry();
    if (mv) items.push(mv);
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

      {showAdd && (
        <AddDownloadModal
          onClose={() => setShowAdd(false)}
          onTorrent={(source) => {
            setShowAdd(false);
            setTorrentSource(source);
          }}
        />
      )}
      {torrentSource && (
        <AddTorrentModal
          source={torrentSource}
          onClose={() => setTorrentSource(null)}
        />
      )}

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
          <div className="dl-f-status">
            <MultiSelect
              ariaLabel="Filter by status"
              caret
              selected={statusSel}
              onToggle={toggleStatus}
              trigger={
                statusSel.has("all") ? (
                  <span className="opt">
                    All<span className="opt-count">{counts.all}</span>
                  </span>
                ) : statusSel.size === 1 ? (
                  <span className="opt">
                    {FILTER_MAP[[...statusSel][0]].label}
                    <span className="opt-count">
                      {counts[[...statusSel][0]]}
                    </span>
                  </span>
                ) : (
                  <span className="opt">{statusSel.size} filters</span>
                )
              }
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
          {store.categories.length > 0 && (
            <div className="dl-f-category">
              <MultiSelect
                ariaLabel="Filter by category"
                caret
                selected={categorySel}
                onToggle={toggleCategory}
                trigger={
                  categorySel.has("")
                    ? "All categories"
                    : categorySel.size === 1
                      ? [...categorySel][0] === "none"
                        ? "Uncategorized"
                        : store.categories.find(
                            (c) => c.id === [...categorySel][0],
                          )?.name ?? "1 category"
                      : `${categorySel.size} categories`
                }
                options={[
                  { value: "", label: "All categories" },
                  { value: "none", label: "Uncategorized" },
                  // Hidden-from-All categories sink to the bottom of the filter
                  // list, regardless of their normal order (stable otherwise).
                  ...[...store.categories]
                    .sort(
                      (a, b) =>
                        Number(a.hidden_from_all) - Number(b.hidden_from_all),
                    )
                    .map((c) => {
                      const tone = c.icon_color || c.color;
                    return {
                      value: c.id,
                      label: (
                        <span className="accent-option">
                          <CategoryIcon
                            icon={c.icon}
                            color={c.color}
                            iconColor={c.icon_color}
                            size={16}
                          />
                          <span
                            style={
                              tone
                                ? { color: categorySwatch(tone) }
                                : undefined
                            }
                          >
                            {c.name}
                          </span>
                        </span>
                      ),
                    };
                  }),
                ]}
              />
            </div>
          )}
          <div className="dl-sort">
              <Select
                value={primary?.key ?? ""}
                ariaLabel="Sort by"
                caret
                onChange={(v) =>
                  v === "" ? clearSort() : sortBy(v as SortKey)
                }
                options={[
                  {
                    value: "",
                    label: <span className="sort-ph">Sort by…</span>,
                  },
                  ...SORT_OPTIONS.map((o) => ({ value: o.key, label: o.label })),
                ]}
              />
              <button
                className="sort-dir"
                onClick={toggleSortDir}
                disabled={!primary}
                aria-label="Toggle sort direction"
                title={
                  primary
                    ? primary.dir === "asc"
                      ? "Ascending"
                      : "Descending"
                    : "Sort direction"
                }
              >
                <span
                  className={`sort-ind${primary?.dir === "asc" ? " asc" : ""}`}
                >
                  <SortArrowIcon size={14} />
                </span>
              </button>
          </div>
          <div className="dl-search-group">
            <input
              className="dl-search selectable"
              type="text"
              placeholder="Filter by name…"
              value={query}
              spellCheck={false}
              onChange={(e) => setQuery(e.target.value)}
            />
            <button
              className="toolbar-add"
              onClick={() => setShowAdd(true)}
              aria-label="Add a download"
              title="Add a download"
            >
              <AddIcon size={18} />
            </button>
          </div>
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
            {displayed.map((t) => (
              <Card
                key={t.id}
                task={t}
                speed={store.speeds[t.id] ?? 0}
                move={store.moves[t.id]}
                expanded={expanded === t.id}
                selected={sel.isSelected(t.id)}
                archived={isArchived}
                columns={columns}
                gridTemplate={gridTemplate}
                categories={store.categories}
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
            moveOpen
              ? "Move to category"
              : menuTasks.length > 1
                ? `${menuTasks.length} selected`
                : undefined
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
      case "ratio":
        return t.received > 0 ? (t.uploaded ?? 0) / t.received : 0;
      case "seeders":
        return t.seeders ?? 0;
      case "leechers":
        return t.leechers ?? 0;
      case "peers":
        return t.peers ?? 0;
      case "uploaded":
        return t.uploaded ?? 0;
      case "upspeed":
        return t.up_speed ?? 0;
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

/** Card CSS vars. `--cat` tints the background from the category's main color;
 *  `--cat-glow` colors the border and the hover/select glow from its icon color
 *  (falling back to the main color). Each is set only when it has a color, so an
 *  uncategorized or colorless card keeps a neutral border and no tint. */
function cardTintStyle(cat?: Category): CSSProperties | undefined {
  if (!cat) return undefined;
  const tint = cat.color ? categorySwatch(cat.color) : null;
  const glowSrc = cat.effects_color || cat.color;
  const glow = glowSrc ? categorySwatch(glowSrc) : null;
  if (!tint && !glow) return undefined;
  const style: Record<string, string> = {};
  if (tint) style["--cat"] = tint;
  if (glow) style["--cat-glow"] = glow;
  return style as CSSProperties;
}

interface CardProps {
  task: Task;
  speed: number;
  /** Relocation progress while the task is Moving, else undefined. */
  move?: MoveProgress;
  expanded: boolean;
  selected: boolean;
  archived: boolean;
  columns: Column[];
  gridTemplate: string;
  categories: Category[];
  onContext: (e: ReactMouseEvent, task: Task) => void;
}

function Card({
  task,
  speed,
  move,
  expanded,
  selected,
  archived,
  columns,
  gridTemplate,
  categories,
  onContext,
}: CardProps) {
  const cat = findCategory(categories, task.category);
  const dl = task.status === "downloading";
  const checking = task.status === "checking";
  const moving = task.status === "moving";
  const seeding = task.status === "seeding";
  // Ratio drives the seeding bar: uploaded ÷ downloaded, 0 before anything's done.
  const ratio = task.received > 0 ? (task.uploaded ?? 0) / task.received : 0;
  // While moving, the bar tracks the file relocation, not the download bytes.
  const movePct =
    moving && move && move.total ? percent(move.moved, move.total) : null;
  const pct = moving ? movePct : percent(task.received, task.total);
  const remaining = task.total != null ? task.total - task.received : null;
  // Indeterminate (sliding) bar only when there's genuinely no percentage to
  // show yet (e.g. a magnet still resolving). Checking reports verified bytes, so
  // it shows a real, filling bar like any other progress.
  const indeterminate =
    pct == null &&
    (dl ||
      moving ||
      checking ||
      task.status === "queued" ||
      task.status === "connecting");
  const live = (dl || moving) && !indeterminate;

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
            {cat && (
              <span className="dl-name-icon">
                <CategoryIcon
                  icon={cat.icon}
                  color={cat.color}
                  iconColor={cat.icon_color}
                  size={15}
                />
              </span>
            )}
            <span className="dl-name-col">
              <span className="dl-name-text">{task.filename}</span>
              {task.kind === "torrent" && (
                <span className="dl-torrent-line">
                  <span
                    className={`tl-metric${dl && speed > 0 ? " tl-down" : ""}`}
                  >
                    <ArrowDown size={12} strokeWidth={2.5} />
                    {formatSpeed(dl ? speed : 0)}
                  </span>
                  <span
                    className={`tl-metric${
                      (task.up_speed ?? 0) > 0 ? " tl-up" : ""
                    }`}
                  >
                    <ArrowUp size={12} strokeWidth={2.5} />
                    {formatSpeed(task.up_speed ?? 0)}
                  </span>
                  <span
                    className={`tl-metric${
                      (task.seeders ?? 0) > 0 ? " tl-seeders" : ""
                    }`}
                  >
                    <Sprout size={12} strokeWidth={2.5} />
                    {task.seeders ?? 0}
                  </span>
                </span>
              )}
            </span>
          </span>
        );
      case "size":
        return (
          <span className="num dim">
            {formatBytes(task.total ?? task.received)}
          </span>
        );
      case "progress":
        // Seeding keeps the download's own bar colour (the app accent) — it
        // doesn't switch tone until seeding actually stops (then it goes to the
        // done/green bar). The bar is split by a hard line into the accent and a
        // darker shade of it: below ratio 1.0 the accent is the base and the
        // darker shade grows from the left to the split (= ratio); at 1.0+ they
        // swap (`.over`) so the darker shade is the base and the accent marks the
        // 1.0 point (= 1/ratio). The ratio itself sits where the percentage would.
        if (seeding) {
          const over = ratio >= 1;
          const overlayW = over ? (ratio > 0 ? 1 / ratio : 0) : ratio;
          return (
            <span className="dl-c-prog">
              <span
                className={`mini-bar ratio${over ? " over" : ""}`}
                style={{ "--p": 1, "--r": Math.min(overlayW, 1) } as CSSProperties}
              >
                <i />
                <b />
              </span>
              <span className="mini-pct ratio" title={`ratio ${ratio.toFixed(2)}`}>
                {ratio.toFixed(2)}
              </span>
            </span>
          );
        }
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
          <span
            className={`dl-status ${STATUS_CLASS[task.status]} st-${task.status}`}
          >
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
      style={cardTintStyle(cat)}
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
          {task.kind === "torrent" ? (
            <TorrentDetail task={task} speed={speed} />
          ) : (
          <div className="dl-detail-grid">
            <div className="detail-item">
              <span className="dk">Downloaded</span>
              <span className="dv">
                {formatBytes(task.received)}
                {task.total != null ? ` of ${formatBytes(task.total)}` : ""}
              </span>
            </div>
            <div className="detail-item right">
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
            <div className="detail-item right">
              <span className="dk">Time spent</span>
              <span className="dv">{formatDuration(task.active_ms)}</span>
            </div>
            {task.backend && (
              <div className="detail-item">
                <span className="dk">Engine</span>
                <span className="dv">{backendLabel(task.backend)}</span>
              </div>
            )}
            {(() => {
              const cat = findCategory(categories, task.category);
              if (!cat) return null;
              return (
                <div className="detail-item right">
                  <span className="dk">Category</span>
                  <span className="dv cat-tag">
                    <CategoryIcon
                      icon={cat.icon}
                      color={cat.color}
                      iconColor={cat.icon_color}
                      size={14}
                    />
                    {cat.name}
                  </span>
                </div>
              );
            })()}
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
          )}
        </div>
      </div>
    </div>
  );
}
