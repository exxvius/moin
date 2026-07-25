// Thin typed wrappers over the engine's command API.
//
// One entry per route; the names match the daemon's `/api/<name>` routes exactly,
// so there's never a question of which call maps to which handler. The handful of
// things only the desktop shell can do still go through Tauri `invoke`.

import { invoke } from "@tauri-apps/api/core";
import { call } from "./engine";
import type {
  AppInfo,
  BackendInfo,
  Category,
  Settings,
  Task,
  ToolStatus,
  TorrentDetails,
  TorrentPreview,
  UpdateInfo,
} from "./types";

export const api = {
  // ---- Shell (Tauri) --------------------------------------------------------
  hideWindow: () => invoke<void>("hide_window"),
  quitApp: () => invoke<void>("quit_app"),
  /** Keep the native close handler's view current — it can't await the engine at
   *  the moment the user clicks the close button, so it reads a cached copy. */
  setQuitPolicy: (closeToTray: boolean, hasActiveTransfers: boolean) =>
    invoke<void>("set_quit_policy", {
      closeToTray,
      hasActiveTransfers,
    }),

  // ---- Engine ---------------------------------------------------------------
  appInfo: () => call<AppInfo>("app_info"),
  addDownload: (url: string, category?: string | null) =>
    call<Task>("add_download", { url, category: category ?? null }),
  prepareTorrent: (source: string) =>
    call<TorrentPreview>("prepare_torrent", { source }),
  addTorrent: (
    source: string,
    dir: string,
    category: string | null,
    selected: number[],
    folder: string | null,
    renames: string[],
  ) =>
    call<Task>("add_torrent", {
      source,
      dir,
      category,
      selected,
      folder,
      renames,
    }),
  mergeTorrentTrackers: (source: string) =>
    call<Task>("merge_torrent_trackers", { source }),
  torrentDetails: (id: string) => call<TorrentDetails>("torrent_details", { id }),
  setTorrentFiles: (id: string, selected: number[]) =>
    call<void>("set_torrent_files", { id, selected }),
  listDownloads: () => call<Task[]>("list_downloads"),
  pauseDownload: (id: string) => call<void>("pause_download", { id }),
  resumeDownload: (id: string) => call<void>("resume_download", { id }),
  startSeeding: (id: string) => call<void>("start_seeding", { id }),
  forceStart: (id: string) => call<void>("force_start", { id }),
  forceRecheck: (id: string) => call<void>("force_recheck", { id }),
  cancelDownload: (id: string) => call<void>("cancel_download", { id }),
  removeDownload: (id: string) => call<void>("remove_download", { id }),
  deleteDownload: (id: string) => call<void>("delete_download", { id }),
  removeDownloads: (ids: string[]) => call<void>("remove_downloads", { ids }),
  deleteDownloads: (ids: string[]) => call<void>("delete_downloads", { ids }),
  retryDownload: (id: string) => call<void>("retry_download", { id }),
  forgetDownload: (id: string) => call<void>("forget_download", { id }),
  getSettings: () => call<Settings>("get_settings"),
  /** Save only the fields that actually changed.
   *
   *  Deliberately a partial update rather than a whole-object save: with more than
   *  one window open, writing the entire settings object means whichever window
   *  saves last silently reverts anything the other one changed in the meantime.
   *  The engine merges these against the current values. */
  saveSettings: (changes: Partial<Settings>) =>
    call<Settings>("patch_settings", changes as Record<string, unknown>),
  listBackends: () => call<BackendInfo[]>("list_backends"),
  regenerateRpcToken: () => call<string>("regenerate_rpc_token"),
  defaultDownloadDir: () => call<string>("default_download_dir"),
  categoryFolder: (category: string | null) =>
    call<string>("category_folder", { category }),
  toolStatus: () => call<ToolStatus>("tool_status"),
  downloadTool: () => call<ToolStatus>("download_tool"),
  setToolPath: (path: string | null) => call<ToolStatus>("set_tool_path", { path }),
  listCategories: () => call<Category[]>("list_categories"),
  suggestCategory: (url: string) => call<string | null>("suggest_category", { url }),
  createCategory: (category: Category) =>
    call<Category[]>("create_category", { category }),
  updateCategory: (category: Category) =>
    call<Category[]>("update_category", { category }),
  deleteCategory: (id: string) => call<Category[]>("delete_category", { id }),
  reorderCategories: (ids: string[]) =>
    call<Category[]>("reorder_categories", { ids }),
  moveToCategory: (ids: string[], category: string | null) =>
    call<void>("move_to_category", { ids, category }),
  checkUpdate: () => call<UpdateInfo>("check_update"),
};
