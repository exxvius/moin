# moin

A download manager for the desktop, built with Rust and Tauri. One queue, one
clean interface, and a companion browser extension that sends your downloads
straight to it.

moin is in active development. Direct HTTP/HTTPS downloading is complete and
solid; BitTorrent and media sites are on the roadmap below.

## Features

- **Fast HTTP/HTTPS downloads.** Files are split across parallel connections and
  reassembled, with per-segment resume that survives a pause, a dropped
  connection, or an app restart.
- **One managed queue.** A concurrency limit you control, live progress and
  speed, and all-time stats. Downloads persist across restarts.
- **Categories.** File downloads into named buckets by rules — file extension,
  size, or URL/name patterns — each with an optional destination folder, colour,
  and icon. New downloads are sorted automatically.
- **Two download engines.** A built-in engine that needs no setup, or
  [aria2c](https://aria2.github.io/) as a drop-in alternative, selectable per
  source. aria2c is fetched from within the app or you can point moin at your own
  binary.
- **Browser extension.** Capture downloads from Chrome, Edge, Brave, and Firefox
  and hand them to moin — cookies and all, so downloads from sites you're signed
  in to work. See [`extension/`](extension/).
- **A considered interface.** A dark-first design with a selectable accent colour
  that recolours the whole app, and a light theme that's equally deliberate.

## Browser extension

The companion extension in [`extension/`](extension/) intercepts downloads in the
browser and sends them to moin over a local-only connection. It captures a link
as you click it (before the browser's save dialog), through a right-click
**"Download with moin"**, and — on Firefox — from server-driven downloads too. If
moin isn't running, it offers to launch it.

Enable it under **Settings → Browser integration** in moin, then load the
extension and pair it with the shown token. Setup and per-browser instructions are
in the [extension README](extension/README.md).

## Nothing is bundled

moin ships small. External tools are never packaged inside it — aria2c today, and
`yt-dlp` and `ffmpeg` when the media engine lands. Fetch the current build from
within the app, or point moin at a binary you already have. If a supplied binary
is too old or missing a capability moin needs, moin flags it and offers to fetch a
current one.

Direct HTTP downloads have no external dependencies at all.

## Install

Grab an installer or the portable Windows build from the
[latest release](https://github.com/exxvius/moin/releases/latest). Installers are
available for Windows, macOS, and Linux, and each browser-extension zip is
attached to the same release.

## Development

```sh
npm install
npm run tauri dev
```

Build a production bundle with `npm run tauri build`. The Rust engine lives in
`src-tauri/src/core` and is deliberately free of any Tauri types, so it can be
tested on its own:

```sh
cd src-tauri && cargo test
```

## Roadmap

- **BitTorrent** — magnet links and `.torrent` files via an embedded
  [librqbit](https://github.com/ikatson/rqbit) client (DHT, seeding, resume), with
  aria2c as an alternative engine.
- **Media sites** — YouTube and similar through `yt-dlp` and `ffmpeg`, with in-app
  tool management and capability checks.
- **Polish** — bandwidth limits, UPnP/NAT-PMP, a system tray, and first-run setup.

## License

[MIT](LICENSE).
