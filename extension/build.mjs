// Assemble per-browser folders under dist/. Chrome/Edge/Brave and Firefox share
// everything but the manifest (service worker vs background scripts, and Firefox's
// add-on id). Run: `node build.mjs`.
//
// For quick Chromium testing you don't even need this — load the extension/ folder
// unpacked as-is. The build is mainly for a clean Firefox folder and for zipping.

import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const dist = join(root, "dist");

/** Copy the shared payload (source + icons) plus a chosen manifest into a target. */
function assemble(name, manifestFile) {
  const out = join(dist, name);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  cpSync(join(root, "src"), join(out, "src"), { recursive: true });
  cpSync(join(root, "icons"), join(out, "icons"), { recursive: true });
  cpSync(join(root, manifestFile), join(out, "manifest.json"));
  console.log(`built dist/${name}`);
}

assemble("chrome", "manifest.json");
assemble("firefox", "manifest.firefox.json");
