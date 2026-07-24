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
  /** The user force-started this torrent: it bypasses the queue limit and never
   *  drops to stalled. Transient (cleared on restart). */
  force_start?: boolean;
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
  /** Content layout the suggested category prefers (modal pre-selects it). */
  suggested_layout: LayoutMode;
  /** Set when this torrent is already in the list (same info hash): the existing
   *  task's id + name, so the add UI can offer to merge trackers. Null when new. */
  duplicate: DuplicateTorrent | null;
}

/** A torrent already in the list that a new add matches by info hash. */
export interface DuplicateTorrent {
  id: string;
  name: string;
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
  /** How often (seconds) watched folders are scanned for dropped torrents. */
  watch_interval_secs: number;
}

export interface BackendInfo {
  id: string;
  label: string;
  http: boolean;
  torrent: boolean;
  available: boolean;
}

/** App identity from the backend. */
export interface AppInfo {
  name: string;
  version: string;
}

/** Result of an update check against the project's GitHub releases. */
export interface UpdateInfo {
  /** The running version. */
  current: string;
  /** Latest published version (no leading "v"), or null if none was found. */
  latest: string | null;
  /** Whether `latest` is newer than `current`. */
  available: boolean;
  /** Release notes for the latest release, if any. */
  notes: string | null;
  /** Direct installer download for this platform, if attached to the release. */
  asset_url: string | null;
  /** The release page — the fallback place to get the update. */
  page_url: string;
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

/** How a torrent's files land on disk relative to the save folder. Mirrors the
 *  add-torrent modal's layout choice. */
export type LayoutMode = "original" | "subfolder" | "flat";

/** One auto-exclude condition on a torrent file. A file matching any rule is
 *  deselected. Tag mirrors the Rust `FileMatch` enum (kebab-case). */
export type FileMatch =
  | { type: "extension"; exts: string[] }
  | { type: "name-pattern"; patterns: string[] }
  | { type: "size-under"; bytes: number }
  | { type: "size-over"; bytes: number };

export type FileMatchType = FileMatch["type"];

/** A canned rename transform (applied to a file name's stem). */
export type PresetKind =
  | "dots-to-spaces"
  | "underscores-to-spaces"
  | "lowercase"
  | "strip-tags";

/** One rename step, applied in order to each file's name. */
export type RenameRule =
  | { type: "replace"; find: string; with: string; regex: boolean }
  | { type: "preset"; kind: PresetKind };

export type RenameRuleType = RenameRule["type"];

/** Per-category torrent automation: how a torrent this category claims is
 *  organized, whatever source it came from. All fields default empty/neutral. */
export interface Automation {
  /** Files matching any rule are deselected (never downloaded). */
  exclude: FileMatch[];
  /** Content layout applied to torrents filed here. */
  layout: LayoutMode;
  /** Rename steps applied in order to each file's name. */
  renames: RenameRule[];
}

/** A named bucket plus the rules that file downloads into it. */
export interface Category {
  id: string;
  name: string;
  /** Accent id for the chip/dot color. */
  color: string;
  /** Optional separate icon color (accent id or "black"); empty inherits `color`. */
  icon_color: string;
  /** Optional separate color for card effects (border/glow); empty inherits `color`. */
  effects_color: string;
  /** Optional icon id from the curated set; null shows the color dot. */
  icon: string | null;
  /** Hide this category's downloads from the "All categories" filter. */
  hidden_from_all: boolean;
  /** Optional save-folder override; null = default download dir. */
  save_dir: string | null;
  /** Seed ratio override for torrents filed here: null inherits the global limit,
   *  0 seeds forever, >0 stops at that ratio. */
  seed_ratio_limit: number | null;
  /** Seed time override in minutes: null inherits the global, 0 = no time limit,
   *  >0 stops seeding that many minutes after completion. */
  seed_time_limit_mins: number | null;
  /** Optional incomplete-staging folder for in-progress downloads (torrent + HTTP);
   *  null downloads straight into the save folder. */
  incomplete_dir: string | null;
  /** Save a copy of each added torrent's .torrent file into this folder; null keeps
   *  no copy. Torrent sources only. */
  torrent_file_dir: string | null;
  /** Move the saved .torrent copy here once the torrent completes (a separate home
   *  for finished torrents); null leaves it in torrent_file_dir. */
  torrent_file_done_dir: string | null;
  /** Which add-methods this category accepts; empty = any source. */
  sources: AddMethodKind[];
  /** Folders watched for dropped .torrent files (auto-added under this category). */
  watch_folders: string[];
  /** Re-add a .torrent downloaded through moin as a torrent when it files here. */
  capture_torrent_downloads: boolean;
  /** Content conditions; all must pass for a download to match. */
  triggers: Trigger[];
  /** Automated sources: download non-matching watch-folder drops uncategorized. */
  fallback_download: boolean;
  /** Priority; lower wins when several match. */
  order: number;
  /** Torrent automation (exclusions, layout, renames). */
  automation: Automation;
}
