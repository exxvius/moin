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
  download_dir: string | null;
}

export interface BackendInfo {
  id: string;
  label: string;
  http: boolean;
  torrent: boolean;
  available: boolean;
}
