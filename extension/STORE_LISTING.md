# Store listing — copy/paste

Fields for the Firefox Add-ons (AMO) submission form. The same text works for the
Chrome Web Store and Edge Add-ons listings.

---

## Name

```
moin — download capture
```

## Summary

(Shown in listings and search; AMO limit is 250 characters. This is ~200.)

```
Send your browser's downloads to moin, the desktop download manager. It catches a link as you click it — with cookies, referer and user-agent so gated files work — or grab any link by right-click. Nothing leaves your machine.
```

## Description

(Markdown is supported. The first 250 characters matter most.)

```
moin — download capture connects your browser to moin, the desktop download manager, so downloads run through moin instead of the browser.

What it does:

- Catches a download the moment you click a link, before the browser's save dialog, and hands it to moin.
- On Firefox, also catches server-triggered downloads (Content-Disposition) before they start.
- Adds a "Download with moin" item to the right-click menu for links, images, audio and video.
- Passes along cookies, referer and user-agent, so downloads from sites you're signed in to work.
- If moin isn't running, it asks before launching it — it never opens the app silently.

Everything talks to moin over a local-only connection (127.0.0.1). No data is sent to the developer or any third party.

Requires the moin desktop app: https://github.com/exxvius/moin
```

---

## Checkboxes

- **This add-on is experimental** — optional. Leave unchecked, or check it if you
  want to signal it's early/beta.
- **This add-on requires payment, non-free services or software, or additional
  hardware** — leave **unchecked**. moin is free and open source; the extension
  needs it, but it isn't a paid or non-free dependency.

## Categories (pick up to 3)

```
Download Management
```

Nothing else on the list is a real fit, so one category is fine.

## Support email

Your call — this is shown publicly on the listing. Use an address you're happy to
publish (a dedicated alias is a good idea rather than a personal inbox).

## Support website

```
https://github.com/exxvius/moin
```

## License

```
MIT License
```

Matches the moin repository's license (Cargo.toml `license = "MIT"`).

---

## Notes for the reviewer (if there's a "Notes to reviewer" box)

```
This extension is a companion to the moin desktop download manager. To test it end
to end you need moin running on the same machine (it exposes a local-only endpoint
on 127.0.0.1). Without moin, the extension will prompt to launch it. Permissions:
"cookies" and host access are used only to replay the browser's own cookies/referer
to the local moin app so authenticated downloads work; nothing is transmitted to
the developer or any third party (data collection is declared as "none").
```
