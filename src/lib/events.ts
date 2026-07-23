// Task event names + a typed subscriber. Names mirror src-tauri/src/events.rs.

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Task, TaskProgress, ToolProgress } from "./types";

export const EV = {
  added: "moin-task-added",
  progressBatch: "moin-task-progress-batch",
  updated: "moin-task-updated",
  removed: "moin-task-removed",
  toolProgress: "moin-tool-progress",
} as const;

export interface TaskHandlers {
  onAdded?: (task: Task) => void;
  /** A coalesced batch of progress ticks (the backend flushes on a timer). */
  onProgress?: (batch: TaskProgress[]) => void;
  onUpdated?: (task: Task) => void;
  onRemoved?: (id: string) => void;
}

/** Subscribe to all task events; returns a single unlisten function. */
export async function subscribeTasks(h: TaskHandlers): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = await Promise.all([
    listen<Task>(EV.added, (e) => h.onAdded?.(e.payload)),
    listen<TaskProgress[]>(EV.progressBatch, (e) => h.onProgress?.(e.payload)),
    listen<Task>(EV.updated, (e) => h.onUpdated?.(e.payload)),
    listen<string>(EV.removed, (e) => h.onRemoved?.(e.payload)),
  ]);
  return () => unlisteners.forEach((u) => u());
}

/** Subscribe to aria2c binary-download progress; returns an unlisten function. */
export function subscribeToolProgress(
  onProgress: (p: ToolProgress) => void,
): Promise<UnlistenFn> {
  return listen<ToolProgress>(EV.toolProgress, (e) => onProgress(e.payload));
}
