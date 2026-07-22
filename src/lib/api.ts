// Thin typed wrappers over Tauri `invoke`.

import { invoke } from "@tauri-apps/api/core";
import type { BackendInfo, Category, Settings, Task, ToolStatus } from "./types";

export const api = {
  addDownload: (url: string, category?: string | null) =>
    invoke<Task>("add_download", { url, category: category ?? null }),
  listDownloads: () => invoke<Task[]>("list_downloads"),
  pauseDownload: (id: string) => invoke<void>("pause_download", { id }),
  resumeDownload: (id: string) => invoke<void>("resume_download", { id }),
  cancelDownload: (id: string) => invoke<void>("cancel_download", { id }),
  removeDownload: (id: string) => invoke<void>("remove_download", { id }),
  deleteDownload: (id: string) => invoke<void>("delete_download", { id }),
  retryDownload: (id: string) => invoke<void>("retry_download", { id }),
  forgetDownload: (id: string) => invoke<void>("forget_download", { id }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) =>
    invoke<void>("set_settings", { settings }),
  listBackends: () => invoke<BackendInfo[]>("list_backends"),
  regenerateRpcToken: () => invoke<string>("regenerate_rpc_token"),
  defaultDownloadDir: () => invoke<string>("default_download_dir"),
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
};
