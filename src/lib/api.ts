// Thin typed wrappers over Tauri `invoke`.

import { invoke } from "@tauri-apps/api/core";
import type { BackendInfo, Settings, Task, ToolStatus } from "./types";

export const api = {
  addDownload: (url: string) => invoke<Task>("add_download", { url }),
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
  defaultDownloadDir: () => invoke<string>("default_download_dir"),
  toolStatus: () => invoke<ToolStatus>("tool_status"),
  downloadTool: () => invoke<ToolStatus>("download_tool"),
  setToolPath: (path: string | null) =>
    invoke<ToolStatus>("set_tool_path", { path }),
};
