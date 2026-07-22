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
}

export interface BackendInfo {
  id: string;
  label: string;
  http: boolean;
  torrent: boolean;
  available: boolean;
}
