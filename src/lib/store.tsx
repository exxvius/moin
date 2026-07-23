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
}

type Action =
  | { type: "LOAD"; tasks: Task[] }
  | { type: "ADDED"; task: Task }
  | { type: "UPDATED"; task: Task }
  | { type: "PROGRESS"; p: TaskProgress }
  | { type: "REMOVED"; id: string };

const initial: State = { tasks: {}, speeds: {}, moves: {} };

function reducer(state: State, action: Action): State {
  switch (action.type) {
    case "LOAD": {
      const tasks: Record<string, Task> = {};
      for (const t of action.tasks) tasks[t.id] = t;
      return { tasks, speeds: {}, moves: {} };
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
      return {
        ...state,
        tasks: { ...state.tasks, [task.id]: task },
        speeds,
        moves,
      };
    }
    case "PROGRESS": {
      const prev = state.tasks[action.p.id];
      if (!prev) return state;
      // A Moving tick reports relocation bytes, not download bytes — keep it out
      // of the task's own received/total so the download progress is preserved.
      if (action.p.status === "moving") {
        return {
          ...state,
          moves: {
            ...state.moves,
            [action.p.id]: { moved: action.p.received, total: action.p.total },
          },
        };
      }
      const next: Task = {
        ...prev,
        received: action.p.received,
        total: action.p.total ?? prev.total,
        status: action.p.status,
        // Live torrent readings ride the same tick.
        up_speed: action.p.up_speed,
        uploaded: action.p.uploaded,
        peers: action.p.peers,
        seeders: action.p.seeders,
        leechers: action.p.leechers,
      };
      return {
        ...state,
        tasks: { ...state.tasks, [next.id]: next },
        speeds: { ...state.speeds, [next.id]: action.p.speed },
      };
    }
    case "REMOVED": {
      const tasks = { ...state.tasks };
      const speeds = { ...state.speeds };
      const moves = { ...state.moves };
      delete tasks[action.id];
      delete speeds[action.id];
      delete moves[action.id];
      return { tasks, speeds, moves };
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
      onProgress: (p) => dispatch({ type: "PROGRESS", p }),
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

  const value = useMemo<StoreValue>(() => {
    const all = Object.values(state.tasks).sort(
      (a, b) => b.created_at - a.created_at,
    );
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
      cancel: (id) => api.cancelDownload(id),
      remove: (id) => api.removeDownload(id),
      delete: (id) => api.deleteDownload(id),
      retry: (id) => api.retryDownload(id),
      forget: (id) => api.forgetDownload(id),
      moveToCategory: (ids, category) => api.moveToCategory(ids, category),
    };
  }, [state, categories]);

  return (
    <StoreContext.Provider value={value}>{children}</StoreContext.Provider>
  );
}

export function useStore(): StoreValue {
  const ctx = useContext(StoreContext);
  if (!ctx) throw new Error("useStore must be used within StoreProvider");
  return ctx;
}
