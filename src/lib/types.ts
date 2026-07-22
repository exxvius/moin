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
  /** Id of the backend that ran this download ("embedded"/"aria2"), else null. */
  backend: string | null;
  /** Id of the category this download is filed under, else null. */
  category: string | null;
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
  /** Smallest piece worth its own connection, in bytes. Drives both engines. */
  min_split_size: number;
  /** Hide the in-progress .part files while downloading (Windows). */
  hide_part_files: boolean;
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

/** How a download entered moin. Manual now; watch methods arrive with automation. */
export type AddMethodKind =
  | "manual-link"
  | "manual-torrent"
  | "watch-folder"
  | "watch-url-file";

/** One content condition. Tag mirrors the Rust `Trigger` enum (kebab-case). */
export type Trigger =
  | { type: "extension"; exts: string[] }
  | { type: "size"; min: number | null; max: number | null }
  | { type: "url-pattern"; patterns: string[] }
  | { type: "name-pattern"; patterns: string[] };

export type TriggerType = Trigger["type"];

/** A named bucket plus the rules that file downloads into it. */
export interface Category {
  id: string;
  name: string;
  /** Accent id for the chip/dot color. */
  color: string;
  /** Optional icon id from the curated set; null shows the color dot. */
  icon: string | null;
  /** Optional save-folder override; null = default download dir. */
  save_dir: string | null;
  /** Which add-methods this category accepts; empty = any source. */
  sources: AddMethodKind[];
  /** Content conditions; all must pass for a download to match. */
  triggers: Trigger[];
  /** Automated sources only (later): download non-matching items uncategorized. */
  fallback_download: boolean;
  /** Priority; lower wins when several match. */
  order: number;
}
