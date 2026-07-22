// The background worker: capture downloads and hand them to moin.
//
// Two capture paths — the browser's own downloads (auto-intercept) and a
// right-click "Download with moin" — funnel through `capture()`, which also owns
// the "moin isn't running, launch it?" flow. When moin is down the user is asked
// first (never launched silently); denying lets the browser download normally.

// Chrome service worker: pull in the shared helpers. Firefox loads shared.js
// itself via the manifest, where `importScripts` doesn't exist — hence the guard.
if (typeof importScripts === "function") {
  importScripts("shared.js");
}

// How long to wait for moin's RPC to answer after we trigger a launch.
const LAUNCH_TIMEOUT_MS = 20000;
// After a decline while moin is down, stop prompting on auto-captured downloads
// for this long, so a user who just wants the browser isn't nagged repeatedly.
const SUPPRESS_AFTER_DECLINE_MS = 60000;

// URLs we deliberately handed back to the browser (the manual-decline fallback),
// so `onCreated` / the response interceptor don't capture them again and loop.
const bypass = new Set();
// Timestamp until which auto-capture stays out of the way after a decline.
let suppressAutoUntil = 0;

// A cached copy of the config so the synchronous webRequest hook can gate on it.
let cachedConfig = { ...DEFAULTS };
loadConfig().then((c) => (cachedConfig = c));
B.storage.onChanged.addListener(() => loadConfig().then((c) => (cachedConfig = c)));

// ---- Context menu (manual capture) --------------------------------------

function setupMenus() {
  B.contextMenus.removeAll(() => {
    B.contextMenus.create({
      id: "moin-download",
      title: "Download with moin",
      contexts: ["link", "image", "video", "audio", "selection"],
    });
  });
}

B.runtime.onInstalled.addListener(setupMenus);
B.runtime.onStartup.addListener(setupMenus);

// Clicking the toolbar icon opens the options page (there's no default popup).
B.action.onClicked.addListener(() => {
  B.runtime.openOptionsPage();
});

B.contextMenus.onClicked.addListener((info, tab) => {
  const url = info.linkUrl || info.srcUrl || info.selectionText;
  if (!isHandoffable(url)) {
    notify("Nothing to download", "That link isn't a direct file moin can fetch.");
    return;
  }
  captureAndFallback(url, tab?.url, tab?.id);
});

// ---- Links caught before the browser started downloading ----------------

// A capture that hands the file back to the browser if moin declines or errors,
// so nothing is lost. Used by the context menu, the content-script link
// interceptor, and (Firefox) the response interceptor — none of which have a
// paused browser download to resume, so each re-issues one on fallback.
async function captureAndFallback(url, referrer, tabId) {
  const outcome = await capture({ url, referrer, tabId });
  if (outcome === "handed") return;
  bypass.add(url);
  B.downloads.download({ url }).catch(() => bypass.delete(url));
}

// The content script pre-empts a click on a downloadable link and sends it here
// before the browser ever requests it — no save dialog.
B.runtime.onMessage.addListener((msg, sender) => {
  if (msg?.type === "moin-capture-link" && isHandoffable(msg.url)) {
    captureAndFallback(msg.url, sender?.tab?.url || msg.referrer, sender?.tab?.id);
  }
});

// ---- Firefox: catch server-driven downloads before the save dialog ------

// Firefox keeps blocking webRequest in MV3, so it can cancel a download the
// server triggers (Content-Disposition: attachment) before the browser's dialog.
// Chrome MV3 removed this for store extensions, so the manifest gates it: only the
// Firefox build declares `webRequestBlocking`.
function setupResponseInterception() {
  const perms = B.runtime.getManifest().permissions || [];
  if (!perms.includes("webRequestBlocking") || !B.webRequest?.onHeadersReceived) return;
  B.webRequest.onHeadersReceived.addListener(
    onHeadersReceived,
    { urls: ["<all_urls>"], types: ["main_frame", "sub_frame"] },
    ["blocking", "responseHeaders"],
  );
}
setupResponseInterception();

function onHeadersReceived(details) {
  if (!cachedConfig.enabled || !cachedConfig.autoCapture) return {};
  // Only GET downloads can be safely re-issued if the user declines; leave the
  // rest (POST form results, etc.) to the browser.
  if (details.method && details.method !== "GET") return {};
  if (!isHandoffable(details.url) || bypass.has(details.url)) return {};
  if (Date.now() < suppressAutoUntil) return {};
  const disposition = (details.responseHeaders || []).find(
    (h) => h.name.toLowerCase() === "content-disposition",
  );
  if (!disposition || !/attachment/i.test(disposition.value || "")) return {};

  // It's a download — take it and cancel the browser's copy.
  captureAndFallback(details.url, details.documentUrl || details.originUrl, details.tabId);
  return { cancel: true };
}

// ---- Auto-intercept the browser's own downloads -------------------------

B.downloads.onCreated.addListener(async (item) => {
  const cfg = await loadConfig();
  if (!cfg.enabled || !cfg.autoCapture) return;
  if (!isHandoffable(item.url)) return;
  if (bypass.has(item.url)) {
    bypass.delete(item.url);
    return; // a fallback download we already chose to leave with the browser
  }
  if (Date.now() < suppressAutoUntil) return; // recently declined; stay quiet
  if (cfg.minBytes > 0 && item.fileSize > 0 && item.fileSize < cfg.minBytes) return;

  // Pause immediately so no bytes land while we decide. If the pause fails the
  // download already finished (or can't be stopped) — leave it with the browser
  // rather than risk moin re-fetching it into a duplicate file.
  const paused = await pause(item.id);
  if (!paused) return;

  const outcome = await capture({
    url: item.url,
    referrer: item.referrer,
  });

  if (outcome === "handed") {
    await cancel(item.id);
    await erase(item.id);
  } else {
    if (outcome === "declined") suppressAutoUntil = Date.now() + SUPPRESS_AFTER_DECLINE_MS;
    await resume(item.id);
  }
});

// ---- The shared capture pipeline ----------------------------------------

/**
 * Try to hand `{ url, referrer }` to moin. Returns:
 *   "handed"   — moin accepted it
 *   "declined" — moin was down and the user chose not to launch it
 *   "failed"   — an error the user was notified about
 */
async function capture({ url, referrer, tabId }) {
  const cfg = await loadConfig();
  if (!cfg.token) {
    notify("moin isn't set up", "Open the extension's options and paste moin's access token.");
    return "failed";
  }

  if (!(await pingMoin(cfg))) {
    // The prompt fires the moin:// launch itself, from the user's click.
    const wantsLaunch = await askToLaunch(url, tabId);
    if (!wantsLaunch) return "declined";
    if (!(await waitForMoin(cfg))) {
      notify("moin didn't start", "Couldn't reach moin after launching it. Is it installed and is moin:// registered?");
      return "failed";
    }
  }

  try {
    await sendToMoin(cfg, { url, referrer });
    notify("Sent to moin", filenameFromUrl(url));
    return "handed";
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    // A 401 means the paired token is stale — point the user at options.
    if (message.includes("401")) {
      notify("moin rejected the token", "Regenerate it in moin and update the extension's options.");
    } else {
      notify("Couldn't send to moin", message);
    }
    return "failed";
  }
}

/** Poll moin's `/ping` until it answers or we give up. */
async function waitForMoin(cfg) {
  const deadline = Date.now() + LAUNCH_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (await pingMoin(cfg)) return true;
    await sleep(400);
  }
  return false;
}

// ---- The launch prompt window -------------------------------------------

/**
 * Ask whether to launch moin. Prefers an in-page prompt on the originating (or
 * active) tab so it appears over the page; falls back to a popup window when the
 * page can't host one (no content script yet, or a restricted page like the new
 * tab / a store page).
 */
async function askToLaunch(url, tabId) {
  const file = filenameFromUrl(url);
  const target = await resolvePromptTab(tabId);
  if (target != null) {
    try {
      const res = await B.tabs.sendMessage(
        target,
        { type: "moin-show-launch-prompt", file },
        { frameId: 0 },
      );
      if (res && typeof res.launch === "boolean") return res.launch;
    } catch {
      // No content script there — fall back to the window prompt below.
    }
  }
  return askToLaunchWindow(file);
}

/** The originating tab id if usable, else the active tab, else null. */
async function resolvePromptTab(tabId) {
  if (typeof tabId === "number" && tabId >= 0) return tabId;
  try {
    const [tab] = await B.tabs.query({ active: true, lastFocusedWindow: true });
    return tab?.id ?? null;
  } catch {
    return null;
  }
}

/** The popup-window prompt — the fallback when an in-page prompt isn't possible. */
function askToLaunchWindow(file) {
  return new Promise((resolve) => {
    const promptUrl =
      B.runtime.getURL("src/prompt.html") + `?file=${encodeURIComponent(file)}`;

    B.windows
      .create({ url: promptUrl, type: "popup", width: 440, height: 260 })
      .then((win) => {
        let settled = false;
        const finish = (launch) => {
          if (settled) return;
          settled = true;
          B.runtime.onMessage.removeListener(onMessage);
          B.windows.onRemoved.removeListener(onRemoved);
          B.windows.remove(win.id).catch(() => {});
          resolve(launch);
        };
        const onMessage = (msg) => {
          // The prompt reports its own window id, which is more reliable than
          // `sender.tab` across browsers.
          if (msg?.type === "moin-launch-decision" && msg.windowId === win.id) {
            finish(!!msg.launch);
          }
        };
        const onRemoved = (closedId) => {
          if (closedId === win.id) finish(false);
        };
        B.runtime.onMessage.addListener(onMessage);
        B.windows.onRemoved.addListener(onRemoved);
      })
      .catch(() => resolve(false));
  });
}

// ---- Small helpers ------------------------------------------------------

function notify(title, message) {
  B.notifications
    .create({
      type: "basic",
      iconUrl: B.runtime.getURL("icons/icon-128.png"),
      title,
      message: message || "",
    })
    .catch(() => {});
}

/** Best-effort display name from a URL — the last path segment, query stripped. */
function filenameFromUrl(url) {
  try {
    const path = new URL(url).pathname;
    const last = path.split("/").filter(Boolean).pop();
    return last ? decodeURIComponent(last) : url;
  } catch {
    return url;
  }
}

const pause = (id) => B.downloads.pause(id).then(() => true).catch(() => false);
const resume = (id) => B.downloads.resume(id).catch(() => {});
const cancel = (id) => B.downloads.cancel(id).catch(() => {});
const erase = (id) => B.downloads.erase({ id }).catch(() => {});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
