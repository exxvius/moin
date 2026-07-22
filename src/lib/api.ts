// Thin typed wrappers over Tauri `invoke`.

import { invoke } from "@tauri-apps/api/core";
import type { BackendInfo, Settings, Task } from "./types";

export const api = {
  addDownload: (url: string) => invoke<Task>("add_download", { url }),
  listDownloads: () => invoke<Task[]>("list_downloads"),
  pauseDownload: (id: string) => invoke<void>("pause_download", { id }),
  resumeDownload: (id: string) => invoke<void>("resume_download", { id }),
  cancelDownload: (id: string) => invoke<void>("cancel_download", { id }),
  removeDownload: (id: string) => invoke<void>("remove_download", { id }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) =>
    invoke<void>("set_settings", { settings }),
  listBackends: () => invoke<BackendInfo[]>("list_backends"),
  defaultDownloadDir: () => invoke<string>("default_download_dir"),
};
