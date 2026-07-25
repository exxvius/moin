// Engine event names + typed subscribers. Names mirror moin-core's events.rs.
//
// Everything about downloads arrives over the engine's shared event stream (see
// engine.ts). The one exception is the quit prompt: that's the desktop shell
// asking its own window a question, and never leaves the process.

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { onEngineEvent } from "./engine";
import type {
  Category,
  Settings,
  Task,
  TaskProgress,
  ToolProgress,
} from "./types";

export const EV = {
  snapshot: "moin-task-snapshot",
  added: "moin-task-added",
  progressBatch: "moin-task-progress-batch",
  updated: "moin-task-updated",
  removed: "moin-task-removed",
  completed: "moin-task-completed",
  toolProgress: "moin-tool-progress",
  settingsChanged: "moin-settings-changed",
  categoriesChanged: "moin-categories-changed",
  confirmQuit: "moin-confirm-quit",
} as const;

/** Subscribe to the "confirm quit" prompt request (close attempted with active
 *  transfers and tray-minimize off); returns an unlisten function. A shell event,
 *  not an engine one. */
export function subscribeConfirmQuit(onConfirm: () => void): Promise<UnlistenFn> {
  return listen(EV.confirmQuit, () => onConfirm());
}

/** Subscribe to just the "a download was added" event — fires only on genuine
 *  new adds, not on the initial snapshot. Returns an unlisten function. */
export function subscribeTaskAdded(onAdded: () => void): Promise<UnlistenFn> {
  return Promise.resolve(onEngineEvent(EV.added, () => onAdded()));
}

export interface TaskHandlers {
  /** The full task list. Arrives first on every connection, and again if the
   *  stream ever had to resynchronise — so it replaces the set, never merges. */
  onSnapshot?: (tasks: Task[]) => void;
  onAdded?: (task: Task) => void;
  /** A coalesced batch of progress ticks (the engine flushes on a timer). */
  onProgress?: (batch: TaskProgress[]) => void;
  onUpdated?: (task: Task) => void;
  onRemoved?: (id: string) => void;
  /** A download finished — the client turns this into an OS notification. */
  onCompleted?: (task: Task) => void;
}

/** Subscribe to all task events; returns a single unlisten function. */
export async function subscribeTasks(h: TaskHandlers): Promise<UnlistenFn> {
  const off = [
    onEngineEvent<Task[]>(EV.snapshot, (tasks) => h.onSnapshot?.(tasks)),
    onEngineEvent<Task>(EV.added, (task) => h.onAdded?.(task)),
    onEngineEvent<TaskProgress[]>(EV.progressBatch, (b) => h.onProgress?.(b)),
    onEngineEvent<Task>(EV.updated, (task) => h.onUpdated?.(task)),
    onEngineEvent<string>(EV.removed, (id) => h.onRemoved?.(id)),
    onEngineEvent<Task>(EV.completed, (task) => h.onCompleted?.(task)),
  ];
  return () => off.forEach((fn) => fn());
}

/** Subscribe to aria2c binary-download progress; returns an unlisten function. */
export function subscribeToolProgress(
  onProgress: (p: ToolProgress) => void,
): Promise<UnlistenFn> {
  return Promise.resolve(onEngineEvent<ToolProgress>(EV.toolProgress, onProgress));
}

/** Settings changed — possibly in another window. Every view showing settings
 *  applies this so two windows can't drift apart. */
export function subscribeSettings(onChange: (s: Settings) => void): () => void {
  return onEngineEvent<Settings>(EV.settingsChanged, onChange);
}

/** The category list changed, from any window. */
export function subscribeCategories(
  onChange: (categories: Category[]) => void,
): () => void {
  return onEngineEvent<Category[]>(EV.categoriesChanged, onChange);
}
