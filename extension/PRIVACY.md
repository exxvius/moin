# Privacy Policy — moin download capture

_Last updated: 2026-07-23_

The **moin — download capture** browser extension does not collect, transmit, or
sell your personal data. It has no analytics, no tracking, and no remote servers.

## What the extension does with your data

The extension's only job is to hand a download from your browser to the **moin
desktop app running on your own computer**. To do that it uses your data as
follows, and only for that purpose:

- **Cookies.** When you send a download to moin, the extension reads the cookies
  for that download's URL and forwards them to moin so downloads from sites you're
  signed in to work. Cookies are read only for the specific URL being downloaded.
- **Page URL / referer.** The address of the page a download came from is sent to
  moin as the referer, so servers that require it will serve the file.
- **Download URL and filename.** The link being downloaded, and the name your
  browser would have given it, are sent to moin.

All of this is sent **only** to moin's local endpoint on your machine
(`http://127.0.0.1`). None of it is sent to the developer, to any third party, or
to any server on the internet.

## What the extension stores

The extension saves its own settings in your browser's local extension storage:
the port and access token it uses to reach the local moin app, and your capture
on/off preferences. This stays on your device and is never transmitted.

## Permissions

Each permission the extension requests is used solely to send downloads to your
local moin app:

- **downloads / contextMenus / notifications** — to catch downloads, offer a
  right-click "Download with moin", and show a short success/error message.
- **cookies / host access** — to read the cookies and URL for the download being
  sent, and to reach moin on `127.0.0.1`.
- **tabs** — to show the "launch moin?" prompt and read the current page's URL as
  the referer.
- **storage** — to save the extension's own settings, described above.

## Remote code

The extension runs only the code bundled in its package. It does not download or
execute any code from the internet.

## Changes

If this policy changes, the updated version will be published in this repository
with a new "last updated" date.

## Contact

Questions or concerns: open an issue at
<https://github.com/exxvius/moin/issues>.
