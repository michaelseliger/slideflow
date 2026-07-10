// Build the `slideflow` CLI and stage it as a Tauri `externalBin` sidecar, so it
// ships *inside* the app bundle. At runtime, Settings → Advanced → "Install
// command line tool" symlinks it onto the user's PATH (the VS Code `code`
// pattern) — the app is the CLI's distribution channel.
//
// Wired into tauri.conf.json's before-commands, so every `pnpm tauri dev`/`build`
// stages a CLI. Runs from `apps/desktop` (Tauri's before-command cwd).
//
// It builds the CLI from the ROOT cargo workspace (repo root) — the CLI depends
// on slideflow-core WITHOUT the desktop's `embeddings` feature, so this is a
// small, pure-Rust build independent of the Tauri host — then copies the binary
// to the path `bundle.externalBin: ["binaries/slideflow-cli"]` expects:
//
//   apps/desktop/src-tauri/binaries/slideflow-cli-<target-triple>[.exe]
//
// Target triple precedence (highest first):
//   1. $SLIDEFLOW_CLI_TARGET — explicit override.
//   2. $TAURI_ENV_TARGET_TRIPLE — the triple Tauri's CLI is building for; it sets
//      this for before{Dev,Build}Command hooks, so `tauri build --target <triple>`
//      stages the *matching* sidecar (otherwise tauri-build panics looking for a
//      triple we never staged).
//   3. The host triple from `rustc -vV`.
//
// `--dev` (passed from beforeDevCommand): dev is a fast path. The app refuses CLI
// installs in dev, so the sidecar only needs to *exist* for tauri-build to resolve
// it — we skip the build entirely when one is already staged, and otherwise build
// the unoptimized debug profile instead of paying a full `--release` recompile of
// the slideflow-core dep tree on every `pnpm tauri dev`.

import { execFileSync } from "node:child_process";
import { mkdirSync, copyFileSync, chmodSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url)); // apps/desktop/scripts
const repoRoot = resolve(here, "..", "..", ".."); // slideflow/
const binariesDir = resolve(here, "..", "src-tauri", "binaries");

const dev = process.argv.includes("--dev");

function hostTriple() {
  const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  const m = out.match(/^host:\s*(.+)$/m);
  if (!m) throw new Error("could not determine host target triple from `rustc -vV`");
  return m[1].trim();
}

const host = hostTriple();
const override = (process.env.SLIDEFLOW_CLI_TARGET || "").trim();
const tauriTriple = (process.env.TAURI_ENV_TARGET_TRIPLE || "").trim();
const triple = override || tauriTriple || host;
const source = override ? "SLIDEFLOW_CLI_TARGET" : tauriTriple ? "TAURI_ENV_TARGET_TRIPLE" : "host";
const cross = triple !== host; // only pass --target when genuinely cross-compiling
const isWindows = triple.includes("windows");
const exe = isWindows ? ".exe" : "";

const destBin = join(binariesDir, `slideflow-cli-${triple}${exe}`);

// Surface the resolved triple + where it came from, so a cross-build mismatch is
// obvious in the log rather than a bare tauri-build "resource not found" panic.
console.log(`[embed-cli] target triple ${triple} (from ${source}${cross ? ", cross-compiling" : ""})`);

// A staged sidecar counts only if non-empty: src-tauri/build.rs creates a 0-byte
// placeholder for bare cargo builds, which must not shadow a real binary here.
function stagedSize(path) {
  try {
    return statSync(path).size;
  } catch {
    return 0;
  }
}

if (dev && stagedSize(destBin) > 0) {
  console.log(`[embed-cli] dev: reusing staged sidecar ${destBin} (skipping build)`);
  process.exit(0);
}

const profile = dev ? "debug" : "release";
const cargoArgs = ["build", "-p", "slideflow-cli"];
if (!dev) cargoArgs.push("--release");
if (cross) cargoArgs.push("--target", triple);

console.log(`[embed-cli] building slideflow-cli (${profile}) for ${triple}${cross ? " (cross)" : ""}…`);
execFileSync("cargo", cargoArgs, { cwd: repoRoot, stdio: "inherit" });

const builtDir = cross
  ? join(repoRoot, "target", triple, profile)
  : join(repoRoot, "target", profile);
const builtBin = join(builtDir, `slideflow${exe}`);

mkdirSync(binariesDir, { recursive: true });
copyFileSync(builtBin, destBin);
if (!isWindows) chmodSync(destBin, 0o755);
console.log(`[embed-cli] staged ${destBin}`);
