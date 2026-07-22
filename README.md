# moin

A single download manager for direct links, torrents, and media sites. They all
share one queue and one interface. Built with Rust and Tauri, with a React
frontend.

## Supported sources

- **Direct HTTP/HTTPS.** Downloads in parallel chunks and resumes after a
  dropped connection.
- **BitTorrent.** Magnet links and `.torrent` files, handled by an embedded
  client ([librqbit](https://github.com/ikatson/rqbit)). No extra software is
  required; DHT, seeding, and resume are all included.
- **Media sites.** YouTube and similar, through `yt-dlp`.

## No bundled binaries

moin doesn't ship `yt-dlp` or `ffmpeg` inside it, which keeps the app small.
Download the current version from within moin when you need it, or point it at a
binary you already have. If the binary you provide is out of date or missing a
capability moin requires, moin flags it and offers to fetch a current build.

Direct downloads and torrents have no external dependencies. Only media-site
downloads rely on `yt-dlp`, plus `ffmpeg` when audio and video need to be merged.

## Interface

A dark-first design with a single accent color that recolors the entire app. The
accent is selectable in settings and works in both light and dark themes. It
defaults to blue.

## Roadmap

Development is happening in stages:

1. Scaffold and theming (current)
2. Direct HTTP downloads: task model, queue, persistence, and live progress
3. BitTorrent: librqbit session, magnet support, and seeding
4. Media sites: yt-dlp and ffmpeg, with in-app tool management and capability
   checks
5. Remaining work: bandwidth limits, system tray, and first-run setup

## Development

```sh
npm install
npm run tauri dev
```

## License

MIT
