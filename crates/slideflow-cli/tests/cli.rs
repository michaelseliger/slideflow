//! End-to-end tests for the `slideflow` CLI.
//!
//! Each test spawns the real built binary (`CARGO_BIN_EXE_slideflow`) against a
//! temp directory, exercising the same engine paths the desktop app uses. The
//! fixture decks are built with `slideflow_core::fixtures` so no on-disk corpus
//! is needed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use slideflow_core::fixtures::{DeckSpec, SlideSpec};
use slideflow_core::pptx::PresentationFile;

/// Absolute path of the built `slideflow` binary under test.
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_slideflow")
}

/// Run `slideflow <args...>` and capture the result.
fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("failed to spawn slideflow binary")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Write two distinguishable fixture decks (alpha: 2 slides, beta: 2 slides)
/// into `dir`, returning their paths. `alpha`'s first slide is titled
/// "Quarterly Revenue" so a `title:revenue` search matches it and not `beta`.
fn seed_decks(dir: &Path) -> (PathBuf, PathBuf) {
    let alpha = dir.join("alpha.pptx");
    let beta = dir.join("beta.pptx");

    DeckSpec::new("Alpha Deck")
        .author("QA")
        .slide(
            SlideSpec::new("Quarterly Revenue")
                .bullets(&["Sales up 20 percent", "Two new markets"]),
        )
        .slide(SlideSpec::new("Appendix").bullets(&["Methodology"]))
        .write_to(&alpha)
        .expect("write alpha.pptx");

    DeckSpec::new("Beta Deck")
        .slide(SlideSpec::new("Marketing Roadmap").bullets(&["Q3 campaign plan"]))
        .slide(SlideSpec::new("Timeline").bullets(&["Milestones"]))
        .write_to(&beta)
        .expect("write beta.pptx");

    (alpha, beta)
}

/// Full happy-path lifecycle: index → search (human + json) → compose → render →
/// stats, all against one temp library.
#[test]
fn full_lifecycle() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let db = dir.join("lib.db");
    let db_s = db.to_str().unwrap();
    let (alpha, _beta) = seed_decks(dir);

    // --- index -------------------------------------------------------------
    let out = run(&["--db", db_s, "index", dir.to_str().unwrap()]);
    assert!(out.status.success(), "index failed: {}", stderr(&out));
    let idx_stdout = stdout(&out);
    // Two decks, four slides total.
    assert!(idx_stdout.contains("2 deck"), "index stdout: {idx_stdout}");
    assert!(idx_stdout.contains("4 slide"), "index stdout: {idx_stdout}");

    // --- search (human) ----------------------------------------------------
    let out = run(&["--db", db_s, "search", "title:revenue"]);
    assert!(out.status.success(), "search failed: {}", stderr(&out));
    let human = stdout(&out);
    assert!(human.contains("alpha.pptx"), "search human output: {human}");
    assert!(!human.contains("beta.pptx"), "search leaked beta: {human}");

    // --- search (json) -----------------------------------------------------
    let out = run(&["--db", db_s, "search", "title:revenue", "--json"]);
    assert!(out.status.success(), "search --json failed: {}", stderr(&out));
    let hits: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("search --json is valid JSON");
    let arr = hits.as_array().expect("json is an array");
    assert_eq!(arr.len(), 1, "expected exactly one hit, got {}", arr.len());
    assert_eq!(arr[0]["deck"]["file_name"], "alpha.pptx");
    assert_eq!(arr[0]["slide"]["slide_index"], 1);

    // --- compose -----------------------------------------------------------
    let out_pptx = dir.join("composed.pptx");
    let pick_a = format!("{}:1", alpha.to_str().unwrap());
    let pick_b = format!("{}:1", _beta.to_str().unwrap());
    let out = run(&[
        "compose",
        out_pptx.to_str().unwrap(),
        &pick_a,
        &pick_b,
        "--title",
        "Combined",
    ]);
    assert!(out.status.success(), "compose failed: {}", stderr(&out));
    assert!(out_pptx.exists(), "compose did not write the output file");
    let composed = PresentationFile::open(&out_pptx).expect("open composed deck");
    assert_eq!(composed.slide_count(), 2, "composed deck should have 2 slides");
    assert!(stdout(&out).contains("2 slide"), "compose stdout: {}", stdout(&out));

    // --- render ------------------------------------------------------------
    let out_svg = dir.join("slide1.svg");
    let out = run(&[
        "render",
        alpha.to_str().unwrap(),
        "1",
        out_svg.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "render failed: {}", stderr(&out));
    let svg = std::fs::read_to_string(&out_svg).expect("read rendered svg");
    assert!(svg.starts_with("<svg"), "render output is not an SVG: {:.40}", svg);

    // --- stats (json) ------------------------------------------------------
    let out = run(&["--db", db_s, "stats", "--json"]);
    assert!(out.status.success(), "stats --json failed: {}", stderr(&out));
    let stats: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("stats --json is valid JSON");
    assert_eq!(stats["deck_count"], 2);
    assert_eq!(stats["slide_count"], 4);
}

/// Searching a database path that does not exist is an operational error (exit 1)
/// with a helpful message on stderr — not a silently-created empty library.
#[test]
fn search_missing_db_errors_cleanly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("nope.db");
    let out = run(&["--db", missing.to_str().unwrap(), "search", "anything"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1");
    let err = stderr(&out);
    assert!(err.contains("no library database"), "stderr: {err}");
    // Nothing on stdout, and no database created.
    assert!(stdout(&out).is_empty(), "unexpected stdout: {}", stdout(&out));
    assert!(!missing.exists(), "search must not create the database");
}

/// A malformed pick spec (`deck.pptx:notanumber`) fails with a clear message and
/// a non-zero exit — never a panic or a truncated stack trace.
#[test]
fn compose_malformed_pick_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let (alpha, _beta) = seed_decks(dir);
    let out_pptx = dir.join("out.pptx");
    let bad = format!("{}:notanumber", alpha.to_str().unwrap());
    let out = run(&["compose", out_pptx.to_str().unwrap(), &bad]);
    assert!(!out.status.success(), "malformed pick should fail");
    // Operational error → exit 1 (usage errors from clap are exit 2; either is
    // acceptable per the spec, but our pick parsing yields 1).
    assert_eq!(out.status.code(), Some(1), "expected exit 1");
    let err = stderr(&out);
    assert!(
        err.contains("invalid pick") && err.contains("not a positive integer"),
        "stderr should explain the bad pick: {err}"
    );
    assert!(!out_pptx.exists(), "no output should be written on a bad pick");
}
