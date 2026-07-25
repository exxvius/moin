// Build the engine daemon and stage it where Tauri expects a sidecar.
//
// `bundle.externalBin` in tauri.conf.json points at `binaries/moin-engine`, and
// Tauri resolves that to `binaries/moin-engine-<target-triple><exe>` at bundle
// time — so the installers place the engine next to the app executable, which is
// exactly where the app looks for it (see src-tauri/src/engine_link.rs).
//
// Run before `tauri build` or `tauri dev`:
//   node scripts/build-engine.mjs [--target <triple>]

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const manifest = join(root, "src-tauri", "Cargo.toml");

const args = process.argv.slice(2);
const targetFlag = args.indexOf("--target");
const target = targetFlag === -1 ? null : args[targetFlag + 1];
const release = !args.includes("--debug");

if (targetFlag !== -1 && !target) {
  console.error("--target needs a triple, e.g. x86_64-pc-windows-msvc");
  process.exit(1);
}

/** The triple Tauri will look for. Ask rustc rather than guessing. */
function hostTriple() {
  const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  const line = out.split("\n").find((l) => l.startsWith("host: "));
  if (!line) throw new Error("couldn't read the host triple from rustc -vV");
  return line.slice("host: ".length).trim();
}

const triple = target ?? hostTriple();
const exe = triple.includes("windows") ? ".exe" : "";

const cargo = ["build", "--manifest-path", manifest, "-p", "moin-daemon"];
if (release) cargo.push("--release");
if (target) cargo.push("--target", target);

console.log(`building the engine for ${triple}…`);
execFileSync("cargo", cargo, { stdio: "inherit" });

// Cargo drops a --target build under target/<triple>/, and a host build directly
// under target/.
const profile = release ? "release" : "debug";
const built = join(
  root,
  "src-tauri",
  "target",
  ...(target ? [target] : []),
  profile,
  `moin-engine${exe}`,
);

const stagedDir = join(root, "src-tauri", "binaries");
const staged = join(stagedDir, `moin-engine-${triple}${exe}`);
mkdirSync(stagedDir, { recursive: true });
copyFileSync(built, staged);
console.log(`staged ${staged}`);
