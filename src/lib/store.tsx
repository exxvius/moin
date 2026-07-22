// Central download store: loads the persisted queue, subscribes to engine
// events, and exposes live tasks + actions to the app.

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  type ReactNode,
} from "react";
import { api } from "./api";
import { subscribeTasks } from "./events";
import type { Task, TaskProgress } from "./types";

interface State {
  /** Tasks by id. */
  tasks: Record<string, Task>;
  /** Live speed (bytes/sec) by id, only while downloading. */
  speeds: Record<string, number>;
}

type Action =
  | { type: "LOAD"; tasks: Task[] }
  | { type: "ADDED"; task: Task }
  | { type: "UPDATED"; task: Task }
  | { type: "PROGRESS"; p: TaskProgress }
  | { type: "REMOVED"; id: string };

const initial: State = { tasks: {}, speeds: {} };

function reducer(state: State, action: Action): State {
  switch (action.type) {
    case "LOAD": {
      const tasks: Record<string, Task> = {};
      for (const t of action.tasks) tasks[t.id] = t;
      return { tasks, speeds: {} };
    }
    case "ADDED":
    case "UPDATED": {
      const { task } = action;
      const speeds = { ...state.speeds };
      // A task that isn't actively downloading has no live speed.
      if (task.status !== "downloading") delete speeds[task.id];
      return { ...state, tasks: { ...state.tasks, [task.id]: task }, speeds };
    }
    case "PROGRESS": {
      const prev = state.tasks[action.p.id];
      if (!prev) return state;
      const next: Task = {
        ...prev,
        received: action.p.received,
        total: action.p.total ?? prev.total,
        status: action.p.status,
      };
      return {
        tasks: { ...state.tasks, [next.id]: next },
        speeds: { ...state.speeds, [next.id]: action.p.speed },
      };
    }
    case "REMOVED": {
      const tasks = { ...state.tasks };
      const speeds = { ...state.speeds };
      delete tasks[action.id];
      delete speeds[action.id];
      return { tasks, speeds };
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
];

interface StoreValue {
  /** Newest first, still in flight (queued/connecting/downloading/paused). */
  active: Task[];
  /** Newest first, finished (completed/failed/canceled). */
  finished: Task[];
  speeds: Record<string, number>;
  add: (url: string) => Promise<void>;
  pause: (id: string) => Promise<void>;
  resume: (id: string) => Promise<void>;
  cancel: (id: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
}

const StoreContext = createContext<StoreValue | null>(null);

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initial);

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
      active: all.filter((t) => ACTIVE.includes(t.status)),
      finished: all.filter((t) => !ACTIVE.includes(t.status)),
      speeds: state.speeds,
      add: async (url) => {
        await api.addDownload(url);
      },
      pause: (id) => api.pauseDownload(id),
      resume: (id) => api.resumeDownload(id),
      cancel: (id) => api.cancelDownload(id),
      remove: (id) => api.removeDownload(id),
    };
  }, [state]);

  return (
    <StoreContext.Provider value={value}>{children}</StoreContext.Provider>
  );
}

export function useStore(): StoreValue {
  const ctx = useContext(StoreContext);
  if (!ctx) throw new Error("useStore must be used within StoreProvider");
  return ctx;
}
