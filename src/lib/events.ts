// Task event names + a typed subscriber. Names mirror src-tauri/src/events.rs.

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Task, TaskProgress } from "./types";

export const EV = {
  added: "moin-task-added",
  progress: "moin-task-progress",
  updated: "moin-task-updated",
  removed: "moin-task-removed",
} as const;

export interface TaskHandlers {
  onAdded?: (task: Task) => void;
  onProgress?: (p: TaskProgress) => void;
  onUpdated?: (task: Task) => void;
  onRemoved?: (id: string) => void;
}

/** Subscribe to all task events; returns a single unlisten function. */
export async function subscribeTasks(h: TaskHandlers): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = await Promise.all([
    listen<Task>(EV.added, (e) => h.onAdded?.(e.payload)),
    listen<TaskProgress>(EV.progress, (e) => h.onProgress?.(e.payload)),
    listen<Task>(EV.updated, (e) => h.onUpdated?.(e.payload)),
    listen<string>(EV.removed, (e) => h.onRemoved?.(e.payload)),
  ]);
  return () => unlisteners.forEach((u) => u());
}
