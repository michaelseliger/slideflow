// Build the `slideflow` CLI and stage it as a Tauri `externalBin` sidecar, so it
// ships *inside* the app bundle. At runtime, Settings → Advanced → "Install
// command line tool" symlinks it onto the user's PATH (the VS Code `code`
// pattern) — the app is the CLI's distribution channel.
//
// Wired into tauri.conf.json's `beforeBuildCommand`, so every `pnpm tauri build`
// embeds a fresh CLI. Runs from `apps/desktop` (Tauri's before-command cwd).
//
// It builds the CLI from the ROOT cargo workspace (repo root) — the CLI depends
// on slideflow-core WITHOUT the desktop's `embeddings` feature, so this is a
// small, pure-Rust build independent of the Tauri host — then copies the binary
// to the path `bundle.externalBin: ["binaries/slideflow-cli"]` expects:
//
//   apps/desktop/src-tauri/binaries/slideflow-cli-<target-triple>[.exe]
//
// Target triple: $SLIDEFLOW_CLI_TARGET when set (release.yml sets it per matrix
// entry for the macOS cross-builds), otherwise the host triple from `rustc`.

import { execFileSync } from "node:child_process";
import { mkdirSync, copyFileSync, chmodSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url)); // apps/desktop/scripts
const repoRoot = resolve(here, "..", "..", ".."); // slideflow/
const binariesDir = resolve(here, "..", "src-tauri", "binaries");

function hostTriple() {
  const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  const m = out.match(/^host:\s*(.+)$/m);
  if (!m) throw new Error("could not determine host target triple from `rustc -vV`");
  return m[1].trim();
}

const host = hostTriple();
const triple = (process.env.SLIDEFLOW_CLI_TARGET || "").trim() || host;
const cross = triple !== host; // only pass --target when genuinely cross-compiling
const isWindows = triple.includes("windows");
const exe = isWindows ? ".exe" : "";

const cargoArgs = ["build", "--release", "-p", "slideflow-cli"];
if (cross) cargoArgs.push("--target", triple);

console.log(`[embed-cli] building slideflow-cli for ${triple}${cross ? " (cross)" : ""}…`);
execFileSync("cargo", cargoArgs, { cwd: repoRoot, stdio: "inherit" });

const builtDir = cross
  ? join(repoRoot, "target", triple, "release")
  : join(repoRoot, "target", "release");
const builtBin = join(builtDir, `slideflow${exe}`);
const destBin = join(binariesDir, `slideflow-cli-${triple}${exe}`);

mkdirSync(binariesDir, { recursive: true });
copyFileSync(builtBin, destBin);
if (!isWindows) chmodSync(destBin, 0o755);
console.log(`[embed-cli] staged ${destBin}`);
