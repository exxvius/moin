// Mirrors the Rust types in src-tauri/src/core (serde uses lowercase enums and
// snake_case fields).

export type TaskKind = "http" | "torrent" | "media";

export type TaskStatus =
  | "queued"
  | "connecting"
  | "downloading"
  | "paused"
  | "completed"
  | "failed"
  | "canceled";

export interface Task {
  id: string;
  kind: TaskKind;
  url: string;
  filename: string;
  dest: string;
  status: TaskStatus;
  total: number | null;
  received: number;
  error: string | null;
  created_at: number;
  updated_at: number;
  /** When it first reached "completed" (ms since epoch), else null. */
  completed_at: number | null;
  /** Hidden from the normal list; kept for stats, shown in the Archive filter. */
  archived: boolean;
  /** Total ms spent actively downloading (for average speed). */
  active_ms: number;
}

export interface TaskProgress {
  id: string;
  received: number;
  total: number | null;
  speed: number;
  status: TaskStatus;
}

export interface Settings {
  http_backend: string;
  torrent_backend: string;
  max_concurrent: number;
  /** Max parallel connections per download (1 = single stream). */
  connections: number;
  download_dir: string | null;
  /** Explicit path to a user-supplied aria2c binary, else null. */
  aria2_path: string | null;
}

export interface BackendInfo {
  id: string;
  label: string;
  http: boolean;
  torrent: boolean;
  available: boolean;
}

/** Which link in the resolve chain provided a managed tool's binary. */
export type ToolSource = "override" | "env" | "managed" | "beside" | "path";

/** aria2c availability snapshot, for the Settings tool row. */
export interface ToolStatus {
  id: string;
  present: boolean;
  path: string | null;
  version: string | null;
  source: ToolSource | null;
  /** Whether this platform can fetch the binary in-app (Windows only). */
  can_fetch: boolean;
}

/** Byte progress while a managed tool downloads its binary. */
export interface ToolProgress {
  received: number;
  total: number | null;
}
