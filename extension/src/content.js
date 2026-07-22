// Link interceptor. Runs on every page and catches a click on a downloadable
// link *before* the browser turns it into a download — so moin gets the URL with
// no browser save dialog. This is the "resolve before capture" path; server-driven
// downloads (no clickable file link) still fall to the background's other hooks.
//
// It stays deliberately conservative: it only pre-empts a plain left click on an
// http(s) link that clearly points at a file, so it never hijacks normal
// navigation. Anything it doesn't recognize is left entirely to the browser.

(() => {
  const B = globalThis.browser ?? globalThis.chrome;

  // Only pre-empt when the extension is on and auto-capture is enabled. Cached
  // from storage so the click handler can decide synchronously.
  let active = false;
  const refresh = () =>
    B.storage.local
      .get({ enabled: true, autoCapture: true })
      .then((c) => {
        active = c.enabled && c.autoCapture;
      })
      .catch(() => {});
  refresh();
  B.storage.onChanged.addListener(refresh);

  // File extensions that read as "download this", not "navigate here". Kept to
  // unambiguous binaries/archives/media/docs so we never swallow a real page.
  const DOWNLOAD_EXTS = new Set([
    "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "tgz", "zst",
    "exe", "msi", "dmg", "pkg", "appimage", "deb", "rpm", "apk",
    "iso", "img", "bin",
    "mp4", "mkv", "webm", "avi", "mov", "flv", "wmv", "m4v",
    "mp3", "flac", "wav", "aac", "ogg", "m4a", "opus",
    "pdf", "epub", "mobi",
    "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp",
    "csv", "torrent",
  ]);

  function looksDownloadable(anchor) {
    let url;
    try {
      url = new URL(anchor.href);
    } catch {
      return false;
    }
    // Never touch blob:/data:/mailto:/etc. — moin can't re-fetch those, and the
    // browser must keep handling them.
    if (url.protocol !== "http:" && url.protocol !== "https:") return false;
    // An explicit download attribute is the strongest "this is a file" signal.
    if (anchor.hasAttribute("download")) return true;
    const path = url.pathname.toLowerCase();
    const dot = path.lastIndexOf(".");
    if (dot < 0) return false;
    return DOWNLOAD_EXTS.has(path.slice(dot + 1));
  }

  document.addEventListener(
    "click",
    (event) => {
      if (!active || event.defaultPrevented) return;
      // Plain left click only — let modified clicks (new tab/window, save-as)
      // and middle clicks fall through to the browser.
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
        return;
      }
      const anchor = event.target.closest?.("a[href]");
      if (!anchor || !looksDownloadable(anchor)) return;

      // Take over before the browser starts the download.
      event.preventDefault();
      event.stopImmediatePropagation();
      B.runtime
        .sendMessage({ type: "moin-capture-link", url: anchor.href, referrer: location.href })
        .catch(() => {});
    },
    true, // capture phase, so we win before the page's own handlers
  );

  // ---- In-page "launch moin?" prompt ------------------------------------
  // The background asks us to show this when moin is down, so the prompt appears
  // over the page instead of in a separate popup window.
  B.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
    if (msg?.type !== "moin-show-launch-prompt") return;
    showLaunchPrompt(msg.file).then((launch) => sendResponse({ launch }));
    return true; // keep the channel open for the async answer
  });

  // Navigate to moin:// via a synthetic anchor click. Done inside a user gesture,
  // this triggers the OS handler (and the browser's one-time "Open moin?" dialog);
  // for a custom scheme the current page is left as-is.
  function openMoinScheme() {
    const a = document.createElement("a");
    a.href = "moin://launch";
    a.style.display = "none";
    (document.body || document.documentElement).appendChild(a);
    a.click();
    a.remove();
  }

  function showLaunchPrompt(file) {
    return new Promise((resolve) => {
      // A shadow root keeps the page's CSS from bleeding into (or breaking) the
      // prompt, and vice versa.
      const host = document.createElement("div");
      host.style.cssText =
        "all:initial;position:fixed;inset:0;z-index:2147483647;";
      const shadow = host.attachShadow({ mode: "open" });
      shadow.innerHTML = PROMPT_MARKUP;
      shadow.querySelector(".mark").src = B.runtime.getURL("icons/icon-128.png");
      shadow.querySelector(".file").textContent = file || "this download";

      const done = (launch) => {
        document.removeEventListener("keydown", onKey, true);
        host.remove();
        resolve(launch);
      };
      const onKey = (e) => {
        if (e.key === "Escape") done(false);
      };

      shadow.querySelector("#moin-launch").addEventListener("click", () => {
        // Fire the moin:// launch here, inside the real click gesture — a custom
        // protocol won't open reliably from the background worker.
        openMoinScheme();
        done(true);
      });
      shadow.querySelector("#moin-cancel").addEventListener("click", () => done(false));
      shadow.querySelector(".backdrop").addEventListener("click", (e) => {
        if (e.target === e.currentTarget) done(false);
      });
      document.addEventListener("keydown", onKey, true);

      (document.body || document.documentElement).appendChild(host);
    });
  }

  const PROMPT_MARKUP = `
    <style>
      .backdrop {
        position: fixed; inset: 0; display: flex; align-items: center;
        justify-content: center; background: rgba(6, 8, 12, 0.5);
        font: 14px/1.5 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
      }
      .card {
        width: 380px; max-width: calc(100vw - 40px);
        background: #171b23; color: #eef1f7;
        border: 1px solid #262c38; border-radius: 14px;
        padding: 22px 22px 18px; box-shadow: 0 18px 50px rgba(0,0,0,0.45);
      }
      .row { display: flex; align-items: center; gap: 12px; margin-bottom: 12px; }
      .mark { width: 34px; height: 34px; border-radius: 9px; }
      h1 { margin: 0; font-size: 16px; font-weight: 650; }
      p { margin: 0 0 6px; color: #9aa4b2; }
      .file {
        margin: 4px 0 18px; padding: 8px 12px; background: #222835;
        border-radius: 8px; font-size: 13px; word-break: break-all;
        max-height: 3.2em; overflow: hidden;
      }
      .actions { display: flex; gap: 10px; justify-content: flex-end; }
      button {
        font: inherit; font-weight: 600; padding: 9px 16px; border-radius: 9px;
        border: 1px solid #262c38; background: #222835; color: #eef1f7; cursor: pointer;
      }
      button.primary { background: #4b93ff; border-color: #4b93ff; color: #fff; }
      button:hover { filter: brightness(1.08); }
    </style>
    <div class="backdrop">
      <div class="card" role="dialog" aria-modal="true">
        <div class="row">
          <img class="mark" alt="" />
          <h1>moin isn't running</h1>
        </div>
        <p>Launch moin and send it this download?</p>
        <div class="file"></div>
        <div class="actions">
          <button id="moin-cancel">Not now</button>
          <button id="moin-launch" class="primary">Launch moin</button>
        </div>
      </div>
    </div>`;
})();
