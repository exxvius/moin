// Shared config + moin API helpers, loaded into the background context.
//
// Written as a classic (non-module) script so one file works in both worlds:
// Chrome loads it via `importScripts` from the service worker; Firefox lists it
// ahead of background.js in the manifest's `scripts`. Both share one global
// scope, so these top-level names are visible to background.js.

// `browser` in Firefox, `chrome` in Chromium — both promise-based in MV3.
const B = globalThis.browser ?? globalThis.chrome;

// Settings live in extension storage. The token/port are the pairing with moin.
const DEFAULTS = {
  enabled: true, // master switch for the extension
  autoCapture: true, // intercept the browser's own downloads
  port: 47653, // must match moin's Settings → Browser integration
  token: "", // bearer token copied from moin
  minBytes: 0, // skip auto-capturing files smaller than this (0 = capture all)
};

async function loadConfig() {
  const stored = await B.storage.local.get(DEFAULTS);
  return { ...DEFAULTS, ...stored };
}

async function saveConfig(patch) {
  await B.storage.local.set(patch);
}

/** Base URL of moin's loopback RPC for the configured port. */
function moinBase(cfg) {
  return `http://127.0.0.1:${cfg.port}`;
}

/** Is moin running and reachable? A short timeout keeps a dead port snappy. */
async function pingMoin(cfg) {
  try {
    const res = await fetchWithTimeout(`${moinBase(cfg)}/ping`, {}, 1500);
    return res.ok;
  } catch {
    return false;
  }
}

/**
 * Probe reachability *and* token validity in one call. Posting an empty body to
 * `/add` distinguishes the two: moin answers 401 for a bad token but 400 ("no
 * URL given") once the token checks out — so a 400 means "reachable and paired".
 * Returns "ok" | "bad-token" | "unreachable".
 */
async function verifyMoin(cfg) {
  let res;
  try {
    res = await fetchWithTimeout(
      `${moinBase(cfg)}/add`,
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${cfg.token}`,
        },
        body: "{}",
      },
      1500,
    );
  } catch {
    return "unreachable";
  }
  if (res.status === 401) return "bad-token";
  // 400 = authed but no URL (expected); any 2xx would also mean authed.
  return "ok";
}

/** Hand a download to moin. Throws on a non-OK reply so callers can react. */
async function sendToMoin(cfg, { url, referrer, filename }) {
  const headers = {};
  const cookie = await cookieHeader(url);
  if (cookie) headers["Cookie"] = cookie;
  if (referrer) headers["Referer"] = referrer;
  headers["User-Agent"] = navigator.userAgent;

  const body = { url, headers };
  // Pass the name the browser would have used, when we know a better one than the
  // URL's last segment; moin sanitizes it and files the download under it.
  if (filename) body.filename = filename;

  const res = await fetch(`${moinBase(cfg)}/add`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${cfg.token}`,
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const detail = await res.text().catch(() => "");
    throw new Error(`moin returned ${res.status}${detail ? `: ${detail}` : ""}`);
  }
  return res.json();
}

/** Cookies moin should replay for `url`, as a `Cookie` header, or null if none. */
async function cookieHeader(url) {
  try {
    const cookies = await B.cookies.getAll({ url });
    if (!cookies.length) return null;
    return cookies.map((c) => `${c.name}=${c.value}`).join("; ");
  } catch {
    return null;
  }
}

/** `fetch` with an abort-based timeout, since a dead port can hang otherwise. */
async function fetchWithTimeout(url, options, ms) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), ms);
  try {
    return await fetch(url, { ...options, signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

/** Only http(s) downloads make sense to hand off — moin can't fetch a browser's
 *  blob:/data:/filesystem URLs, which only exist inside that browser. */
function isHandoffable(url) {
  return typeof url === "string" && /^https?:\/\//i.test(url);
}

/** The filename from a Content-Disposition header value, or null. Handles the
 *  RFC 5987 `filename*=UTF-8''…` form and the plain quoted/unquoted `filename=`. */
function contentDispositionName(value) {
  if (!value) return null;
  const star = /filename\*=\s*(?:UTF-8|utf-8)''([^;]+)/i.exec(value);
  if (star) {
    try {
      return decodeURIComponent(star[1].trim());
    } catch {
      // Malformed encoding — fall through to the plain form.
    }
  }
  const plain = /filename=\s*"?([^";]+)"?/i.exec(value);
  return plain ? plain[1].trim() : null;
}

/** Last path segment of a filesystem path (handles / and \), or null. */
function basename(path) {
  if (!path) return null;
  const seg = path.split(/[\\/]/).pop();
  return seg && seg.trim() ? seg.trim() : null;
}
