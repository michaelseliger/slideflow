use std::fs;
use std::path::Path;

fn main() {
    ensure_sidecar_placeholder();
    tauri_build::build();
}

/// Ensure the `externalBin` sidecar exists before `tauri_build::build()` resolves it.
///
/// `binaries/` is gitignored and the real `slideflow-cli` is staged only by
/// `apps/desktop/scripts/embed-cli.mjs`, which tauri runs from its
/// `before{Dev,Build}Command` hooks. A bare `cargo check`/`build`/`test` (or
/// rust-analyzer) on a fresh clone never runs that hook, so `tauri_build::build()`
/// would otherwise panic with `ResourcePathNotFound("binaries/slideflow-cli-<triple>")`.
/// Create an empty placeholder so plain cargo/IDE workflows compile on a fresh
/// checkout. It is never bundled: `tauri build` always stages the real binary via
/// the before-command first, and only `tauri build` bundles at all.
fn ensure_sidecar_placeholder() {
    // Match tauri-build's resolution exactly: the target triple is cargo's `TARGET`
    // (set for every build script), and the sidecar filename gets a `.exe` suffix on
    // Windows targets — see tauri_utils::resources::external_binaries.
    let Ok(target) = std::env::var("TARGET") else {
        return;
    };
    let ext = if target.contains("windows") { ".exe" } else { "" };
    let sidecar = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(format!("slideflow-cli-{target}{ext}"));
    if sidecar.exists() {
        return; // real binary already staged (dev/build flows) — leave it untouched.
    }
    if let Some(parent) = sidecar.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::File::create(&sidecar);
}
