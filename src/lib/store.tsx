// Central download store: loads the persisted queue, subscribes to engine
// events, and exposes live tasks + actions to the app.

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useState,
  type Dispatch,
  type ReactNode,
  type SetStateAction,
} from "react";
import { api } from "./api";
import { subscribeTasks } from "./events";
import type { Category, Task, TaskProgress } from "./types";

/** How far along a file relocation is, while a task is in the Moving state. */
export interface MoveProgress {
  moved: number;
  total: number | null;
}

interface State {
  /** Tasks by id. */
  tasks: Record<string, Task>;
  /** Live speed (bytes/sec) by id, only while downloading. */
  speeds: Record<string, number>;
  /** Relocation progress by id, only while the task is Moving. */
  moves: Record<string, MoveProgress>;
  /** Bumps only when the task *set* changes (add/remove/load), so the sorted
   *  order can be memoized and never re-sorted on a progress tick. */
  version: number;
}

type Action =
  | { type: "LOAD"; tasks: Task[] }
  | { type: "ADDED"; task: Task }
  | { type: "UPDATED"; task: Task }
  // Progress ticks are coalesced and applied a whole frame's worth at a time, so
  // the big `tasks` object is spread once per frame instead of once per tick.
  | { type: "PROGRESS_BATCH"; updates: TaskProgress[] }
  | { type: "REMOVED"; id: string };

const initial: State = { tasks: {}, speeds: {}, moves: {}, version: 0 };

/** Fold one progress reading into the task/speed/move maps, cloning each map at
 *  most once per batch (the maps passed in are the working copies). */
function applyProgress(
  p: TaskProgress,
  tasks: Record<string, Task>,
  speeds: Record<string, number>,
  moves: Record<string, MoveProgress>,
): void {
  const prev = tasks[p.id];
  if (!prev) return;
  // A Moving tick reports relocation bytes, not download bytes — keep it out of
  // the task's own received/total so the download progress is preserved.
  if (p.status === "moving") {
    moves[p.id] = { moved: p.received, total: p.total };
    return;
  }
  tasks[p.id] = {
    ...prev,
    received: p.received,
    total: p.total ?? prev.total,
    status: p.status,
    up_speed: p.up_speed,
    uploaded: p.uploaded,
    peers: p.peers,
    seeders: p.seeders,
    leechers: p.leechers,
  };
  speeds[p.id] = p.speed;
}

function reducer(state: State, action: Action): State {
  switch (action.type) {
    case "LOAD": {
      const tasks: Record<string, Task> = {};
      for (const t of action.tasks) tasks[t.id] = t;
      return { tasks, speeds: {}, moves: {}, version: state.version + 1 };
    }
    case "ADDED":
    case "UPDATED": {
      const { task } = action;
      const speeds = { ...state.speeds };
      // A task that isn't actively downloading has no live speed.
      if (task.status !== "downloading") delete speeds[task.id];
      // Relocation progress only lives while the task is Moving.
      const moves = { ...state.moves };
      if (task.status !== "moving") delete moves[task.id];
      // A new id (ADDED, or an UPDATED for something not yet loaded) changes the
      // set, so the sort order must be recomputed.
      const isNew = state.tasks[task.id] === undefined;
      return {
        tasks: { ...state.tasks, [task.id]: task },
        speeds,
        moves,
        version: isNew ? state.version + 1 : state.version,
      };
    }
    case "PROGRESS_BATCH": {
      // Clone each map lazily — only if a batched update actually touches it.
      let tasks = state.tasks;
      let speeds = state.speeds;
      let moves = state.moves;
      for (const p of action.updates) {
        if (!(p.id in state.tasks)) continue;
        if (p.status === "moving") {
          if (moves === state.moves) moves = { ...state.moves };
        } else {
          if (tasks === state.tasks) tasks = { ...state.tasks };
          if (speeds === state.speeds) speeds = { ...state.speeds };
        }
        applyProgress(p, tasks, speeds, moves);
      }
      if (tasks === state.tasks && speeds === state.speeds && moves === state.moves)
        return state;
      return { ...state, tasks, speeds, moves };
    }
    case "REMOVED": {
      if (!(action.id in state.tasks)) return state;
      const tasks = { ...state.tasks };
      const speeds = { ...state.speeds };
      const moves = { ...state.moves };
      delete tasks[action.id];
      delete speeds[action.id];
      delete moves[action.id];
      return { tasks, speeds, moves, version: state.version + 1 };
    }
    default:
      return state;
  }
}

const ACTIVE: Task["status"][] = [
  "queued",
  "connecting",
  "downloading",
  "paused",
  "moving",
];

interface StoreValue {
  /** Everything, newest first. */
  all: Task[];
  /** Newest first, still in flight (queued/connecting/downloading/paused). */
  active: Task[];
  /** Newest first, finished (completed/failed/canceled). */
  finished: Task[];
  speeds: Record<string, number>;
  /** Relocation progress by id, only while a task is Moving. */
  moves: Record<string, MoveProgress>;
  /** Categories in priority order. */
  categories: Category[];
  /** Replace the category list (accepts a value or an updater fn). */
  setCategories: Dispatch<SetStateAction<Category[]>>;
  add: (url: string, category?: string | null) => Promise<void>;
  pause: (id: string) => Promise<void>;
  resume: (id: string) => Promise<void>;
  startSeeding: (id: string) => Promise<void>;
  forceStart: (id: string) => Promise<void>;
  cancel: (id: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
  delete: (id: string) => Promise<void>;
  retry: (id: string) => Promise<void>;
  forget: (id: string) => Promise<void>;
  /** Move downloads to a category (null = uncategorized). */
  moveToCategory: (ids: string[], category: string | null) => Promise<void>;
}

const StoreContext = createContext<StoreValue | null>(null);

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initial);
  const [categories, setCategories] = useState<Category[]>([]);

  useEffect(() => {
    api.listCategories().then(setCategories).catch(() => {});
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    subscribeTasks({
      onAdded: (task) => dispatch({ type: "ADDED", task }),
      // The backend already coalesces ticks and flushes on a timer, so a batch
      // arrives ready to apply in one dispatch.
      onProgress: (batch) => dispatch({ type: "PROGRESS_BATCH", updates: batch }),
      onUpdated: (task) => dispatch({ type: "UPDATED", task }),
      onRemoved: (id) => dispatch({ type: "REMOVED", id }),
    }).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });

    api.listDownloads().then((tasks) => dispatch({ type: "LOAD", tasks }));

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // The newest-first id order only depends on the task set, so it's memoized on
  // `version` and never re-sorted on a progress tick.
  const order = useMemo(
    () =>
      Object.values(state.tasks)
        .sort((a, b) => b.created_at - a.created_at)
        .map((t) => t.id),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [state.version],
  );

  const value = useMemo<StoreValue>(() => {
    const all = order
      .map((id) => state.tasks[id])
      .filter((t): t is Task => t != null);
    return {
      all,
      active: all.filter((t) => ACTIVE.includes(t.status)),
      finished: all.filter((t) => !ACTIVE.includes(t.status)),
      speeds: state.speeds,
      moves: state.moves,
      categories,
      setCategories,
      add: async (url, category) => {
        await api.addDownload(url, category);
      },
      pause: (id) => api.pauseDownload(id),
      resume: (id) => api.resumeDownload(id),
      startSeeding: (id) => api.startSeeding(id),
      forceStart: (id) => api.forceStart(id),
      cancel: (id) => api.cancelDownload(id),
      remove: (id) => api.removeDownload(id),
      delete: (id) => api.deleteDownload(id),
      retry: (id) => api.retryDownload(id),
      forget: (id) => api.forgetDownload(id),
      moveToCategory: (ids, category) => api.moveToCategory(ids, category),
    };
  }, [state, categories, order]);

  return (
    <StoreContext.Provider value={value}>{children}</StoreContext.Provider>
  );
}

export function useStore(): StoreValue {
  const ctx = useContext(StoreContext);
  if (!ctx) throw new Error("useStore must be used within StoreProvider");
  return ctx;
}
