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

## Chrome Web Store — Privacy practices tab

### Single purpose

```
moin — download capture has one purpose: to hand the browser's downloads to the moin desktop app running on the same computer, so downloads are managed by moin instead of the browser.
```

### Permission justifications

**contextMenus**

```
Adds a single "Download with moin" item to the right-click menu for links, images, audio, and video, so the user can explicitly send a link to moin.
```

**downloads**

```
Used to catch a download the browser starts and hand it to moin instead: the extension pauses the browser's download, sends the URL to moin, then cancels the browser copy (or resumes it if moin declines). Nothing beyond the item being captured is read.
```

**cookies**

```
When a download is sent to moin, the extension reads the cookies for that download's URL and forwards them to the local moin app, so downloads from sites the user is signed in to succeed. Cookies are read only for the URL being downloaded and are sent only to moin on 127.0.0.1.
```

**storage**

```
Stores the extension's own settings locally: the port and access token used to reach the local moin app, and the on/off toggles for capturing. No browsing data is stored.
```

**notifications**

```
Shows a short confirmation when a download has been sent to moin, or an error if it couldn't be (for example, moin isn't running or the access token is wrong).
```

**tabs**

```
Used to show the "launch moin?" prompt on the current tab, and to read the active tab's page URL as the referer for a captured download. No tab browsing history is collected.
```

**Host permission use** (127.0.0.1 and broad site access)

```
Access to 127.0.0.1 is required to talk to the moin desktop app's local endpoint. Broad site access is required because a download can come from any website: the extension reads that site's cookies and the page URL only for the specific download being sent to moin. No page content is collected, and nothing is sent to any remote server.
```

### Remote code

Select **"No, I am not using remote code."** All of the extension's code is bundled
in the package; it only exchanges JSON with the local moin app and never fetches or
executes remotely-hosted code.

### Data usage

The extension does **not** collect user data for the developer or any third party.
Cookies and page URLs are read only to forward a download to the moin app on the
user's own machine (127.0.0.1) — nothing leaves the device. In the data-collection
questions, indicate that no data is sold or transferred for purposes unrelated to
the item's core function, then **tick the certification** that data usage complies
with the Developer Program Policies.

### Contact email

You must add and **verify** a contact email on the **Settings** page before you can
publish — that's your own action; pick an address you're willing to publish.

## Notes for the reviewer (if there's a "Notes to reviewer" box)

```
This extension is a companion to the moin desktop download manager. To test it end
to end you need moin running on the same machine (it exposes a local-only endpoint
on 127.0.0.1). Without moin, the extension will prompt to launch it. Permissions:
"cookies" and host access are used only to replay the browser's own cookies/referer
to the local moin app so authenticated downloads work; nothing is transmitted to
the developer or any third party (data collection is declared as "none").
```
