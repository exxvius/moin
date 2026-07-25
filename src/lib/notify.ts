// OS notification when a download finishes.
//
// This used to happen in Rust, back when the engine lived inside the app. The
// engine is its own process now and has no desktop session to post to, so it
// reports the completion and whichever client is on screen shows it. The engine
// still decides *whether* to fire — it only emits the event when the user has
// notifications switched on — so there's no second check here.

import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

/** Resolved once and reused; asking on every completion would be silly. */
let allowed: Promise<boolean> | null = null;

function permission(): Promise<boolean> {
  allowed ??= (async () => {
    try {
      if (await isPermissionGranted()) return true;
      return (await requestPermission()) === "granted";
    } catch {
      return false;
    }
  })();
  return allowed;
}

/** Best-effort: a denied permission is a no-op, never an error the user sees. */
export async function notifyComplete(filename: string): Promise<void> {
  try {
    if (!(await permission())) return;
    sendNotification({ title: "Download finished", body: filename });
  } catch {
    // Notifications are a nicety; never let one break the download flow.
  }
}
