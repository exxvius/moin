// Mirrors the Rust types in src-tauri/src/core (serde uses lowercase enums and
// snake_case fields).

export type TaskKind = "http" | "torrent" | "media";

export type TaskStatus =
  | "queued"
  | "connecting"
  | "checking"
  | "downloading"
  | "paused"
  | "moving"
  | "completed"
  | "seeding"
  | "failed"
  | "stalled"
  | "canceled";

export type TorrentSource = "magnet" | "file";

/** One file inside a torrent. */
export interface TorrentFile {
  /** Path relative to the torrent's output folder, using "/" separators. */
  path: string;
  size: number;
  /** Whether this file is picked for download. */
  selected: boolean;
  /** Bytes fetched for this file so far. */
  received: number;
}

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

  // Torrent-only fields (defaults/empty on HTTP + media tasks).
  /** BitTorrent info hash (hex), once known. */
  info_hash?: string | null;
  /** Whether the torrent came from a magnet or a .torrent file. */
  torrent_source?: TorrentSource | null;
  /** Files inside the torrent, with per-file size, selection, and progress. */
  files?: TorrentFile[];
  /** Total bytes uploaded to peers (accumulates; feeds ratio + stats). */
  uploaded?: number;
  /** Live swarm readings (0 when not actively transferring). */
  seeders?: number;
  leechers?: number;
  peers?: number;
  /** Upload speed in bytes/sec (live). */
  up_speed?: number;
}

/** One connected peer, for the detail panel's Peers tab. */
export interface PeerInfo {
  addr: string;
  state: string;
  downloaded: number;
}

/** Live per-torrent detail the expanded card polls while open. */
export interface TorrentDetails {
  files: TorrentFile[];
  peers: PeerInfo[];
  trackers: string[];
  /** Have-pieces bucketed into ≤200 segments (fraction held per segment). */
  pieces: number[];
  /** Distributed copies reachable in the swarm (qBittorrent's "availability"). */
  availability: number;
}

/** A torrent resolved to its file list, for the add-torrent modal. */
export interface TorrentPreview {
  info_hash: string;
  name: string | null;
  total: number;
  files: TorrentFile[];
  /** Category a rule would file it under (modal pre-selects it), else null. */
  suggested_category: string | null;
  /** Folder the download would default to (modal shows + lets you override). */
  default_dir: string;
}

export interface TaskProgress {
  id: string;
  received: number;
  total: number | null;
  speed: number;
  status: TaskStatus;
  /** Torrent-only live readings (0 for HTTP). */
  up_speed: number;
  uploaded: number;
  peers: number;
  seeders: number;
  leechers: number;
}

/** What moving a download to another category does to its file. */
export type CategoryChangeBehavior = "change-only" | "move-file";

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
  /** Whether moving a download to a category also relocates its file. */
  category_change: CategoryChangeBehavior;
  /** Seconds of no data before a transfer is marked stalled. 0 = never. */
  stall_timeout_secs: number;
  /** Seconds to wait to establish a connection. 0 = OS default. */
  connect_timeout_secs: number;
  download_dir: string | null;
  /** Explicit path to a user-supplied aria2c binary, else null. */
  aria2_path: string | null;
  /** Whether the loopback RPC server (the browser extension's entry point) runs.
   *  Toggling it or changing the port applies on the next app start. */
  rpc_enabled: boolean;
  /** Loopback port the RPC server binds. Applied at startup. */
  rpc_port: number;
  /** Bearer token the browser extension must present. Read live, so regenerating
   *  it takes effect immediately. */
  rpc_token: string;
  /** Stop seeding a torrent once its ratio reaches this. 0 = seed forever. */
  seed_ratio_limit: number;
  /** Stop seeding a torrent this many minutes after it finishes. 0 = no limit. */
  seed_time_limit_mins: number;
  /** TCP port the torrent engine listens on for peers. Applied on next start. */
  torrent_listen_port: number;
  /** Whether the torrent DHT runs (peer discovery without a tracker). Next start. */
  torrent_dht: boolean;
  /** Whether UPnP/NAT-PMP port forwarding is attempted. Applied on next start. */
  torrent_upnp: boolean;
  /** Torrent download cap in bytes/sec, 0 = unlimited. Applies live. */
  torrent_download_limit: number;
  /** Torrent upload cap in bytes/sec, 0 = unlimited. Applies live. */
  torrent_upload_limit: number;
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
  | "browser-capture"
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
