// Thin typed wrappers over Tauri `invoke`.

import { invoke } from "@tauri-apps/api/core";
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
  appInfo: () => invoke<AppInfo>("app_info"),
  addDownload: (url: string, category?: string | null) =>
    invoke<Task>("add_download", { url, category: category ?? null }),
  prepareTorrent: (source: string) =>
    invoke<TorrentPreview>("prepare_torrent", { source }),
  addTorrent: (
    source: string,
    dir: string,
    category: string | null,
    selected: number[],
    folder: string | null,
    renames: string[],
  ) =>
    invoke<Task>("add_torrent", {
      source,
      dir,
      category,
      selected,
      folder,
      renames,
    }),
  torrentDetails: (id: string) =>
    invoke<TorrentDetails>("torrent_details", { id }),
  setTorrentFiles: (id: string, selected: number[]) =>
    invoke<void>("set_torrent_files", { id, selected }),
  listDownloads: () => invoke<Task[]>("list_downloads"),
  pauseDownload: (id: string) => invoke<void>("pause_download", { id }),
  resumeDownload: (id: string) => invoke<void>("resume_download", { id }),
  startSeeding: (id: string) => invoke<void>("start_seeding", { id }),
  forceStart: (id: string) => invoke<void>("force_start", { id }),
  cancelDownload: (id: string) => invoke<void>("cancel_download", { id }),
  removeDownload: (id: string) => invoke<void>("remove_download", { id }),
  deleteDownload: (id: string) => invoke<void>("delete_download", { id }),
  removeDownloads: (ids: string[]) => invoke<void>("remove_downloads", { ids }),
  deleteDownloads: (ids: string[]) => invoke<void>("delete_downloads", { ids }),
  retryDownload: (id: string) => invoke<void>("retry_download", { id }),
  forgetDownload: (id: string) => invoke<void>("forget_download", { id }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) =>
    invoke<void>("set_settings", { settings }),
  listBackends: () => invoke<BackendInfo[]>("list_backends"),
  regenerateRpcToken: () => invoke<string>("regenerate_rpc_token"),
  defaultDownloadDir: () => invoke<string>("default_download_dir"),
  categoryFolder: (category: string | null) =>
    invoke<string>("category_folder", { category }),
  toolStatus: () => invoke<ToolStatus>("tool_status"),
  downloadTool: () => invoke<ToolStatus>("download_tool"),
  setToolPath: (path: string | null) =>
    invoke<ToolStatus>("set_tool_path", { path }),
  listCategories: () => invoke<Category[]>("list_categories"),
  suggestCategory: (url: string) =>
    invoke<string | null>("suggest_category", { url }),
  createCategory: (category: Category) =>
    invoke<Category[]>("create_category", { category }),
  updateCategory: (category: Category) =>
    invoke<Category[]>("update_category", { category }),
  deleteCategory: (id: string) => invoke<Category[]>("delete_category", { id }),
  reorderCategories: (ids: string[]) =>
    invoke<Category[]>("reorder_categories", { ids }),
  moveToCategory: (ids: string[], category: string | null) =>
    invoke<void>("move_to_category", { ids, category }),
  checkUpdate: () => invoke<UpdateInfo>("check_update"),
};
