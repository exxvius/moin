# moin — browser download capture

A companion extension that hands your browser's downloads to [moin](../). It
captures downloads three ways, from earliest to last-resort:

- **At the click (no save dialog)** — a content script pre-empts a click on a
  downloadable link *before* the browser requests it, so moin gets the URL with no
  browser save window. This is the fast path most downloads take.
- **At the response, Firefox only** — Firefox keeps blocking `webRequest` in MV3,
  so a server-driven download (`Content-Disposition: attachment`) is cancelled
  before its dialog and handed to moin.
- **After the fact (fallback)** — for anything the above miss, it intercepts the
  browser's own download entry (`downloads.onCreated`), pauses, and hands it over.
  On Chrome this can't beat the save dialog (MV3 removed the needed hook), so for
  server-driven downloads either the click path catches it or you turn off Chrome's
  "Ask where to save each file before downloading".

All paths include cookies, referer, and user-agent, so logged-in / gated
downloads work. You can also always right-click a link/image/video → **Download
with moin**.

If moin isn't running when you capture something, the extension asks whether to
launch it (via moin's `moin://` handler), waits for it to come up, then queues the
download. Decline and the browser downloads it normally.

## Setup

1. In **moin → Settings → Browser integration**, make sure it's enabled and copy
   the **access token** (note the port, default `47653`).
2. Load the extension (below).
3. Open the extension's **options**, paste the token, set the port if you changed
   it, and click **Test connection** — it should say "Connected to moin ✓".

## Load it (development)

### Chrome / Edge / Brave

1. Go to `chrome://extensions` (or `edge://extensions`).
2. Turn on **Developer mode**.
3. **Load unpacked** → select this `extension/` folder.

### Firefox

Firefox needs its own manifest (background scripts instead of a service worker):

```
node build.mjs
```

Then in `about:debugging#/runtime/this-firefox` → **Load Temporary Add-on…** →
pick `dist/firefox/manifest.json`.

> Firefox may ask you to grant host access the first time it reads cookies or
> reaches moin — allow it. Temporary add-ons are removed when Firefox restarts.

## How it talks to moin

Everything goes over moin's **loopback-only** RPC (`http://127.0.0.1:<port>`),
authenticated with the access token. The download itself — URL plus the captured
`Cookie` / `Referer` / `User-Agent` headers — is POSTed to `/add`. The `moin://`
launch only wakes moin up; no cookies or token ever ride in that URL.

## Notes & limits

- The first `moin://` launch may also show the browser's own "Open moin?" prompt.
  Tick "always allow" to skip it next time.
- Streaming media, `blob:`, and `data:` downloads can't be handed off (moin can't
  re-fetch a URL that only exists inside the browser) — those fall through to the
  browser.
- moin currently names files from the URL; a browser-provided filename isn't sent
  yet.

## Layout

```
extension/
  manifest.json            Chrome/Edge/Brave (MV3, service worker)
  manifest.firefox.json    Firefox (MV3, background scripts)
  build.mjs                assembles dist/chrome and dist/firefox
  icons/                   moin's icon
  src/
    shared.js              config + moin API (ping / verify / send), cookie header
    content.js             click interceptor + in-page "launch moin?" prompt
    background.js          context menu, link/response/download intercept, launch
    prompt.html/.js        fallback popup-window prompt (restricted pages)
    options.html/.js/.css  pairing (port + token) and capture toggles
```

Clicking the toolbar icon opens these options. The "launch moin?" prompt appears
in-page over the current tab; on a page that can't host it (a freshly opened tab
before the content script loads, or a browser page like the store) it falls back
to a small popup window.
