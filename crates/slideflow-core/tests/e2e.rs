//! End-to-end: build decks on disk → index → search → compose → re-open.
//!
//! Also exercises the real-file corpus when `SLIDEFLOW_CORPUS` points at a
//! directory of .pptx files (CI/dev machines), asserting indexing speed and
//! compose integrity on large decks.

use std::path::Path;

use slideflow_core::fixtures::{DeckSpec, SlideSpec};
use slideflow_core::index::Library;
use slideflow_core::model::{ScanEvent, SearchFilters, SlidePick};
use slideflow_core::pptx::{compose, ComposeOptions, PresentationFile};
use slideflow_core::render::{render_slide_svg, RenderOptions};

fn write_fixture_decks(dir: &Path) {
    DeckSpec::new("Q3 Business Review")
        .author("Alice")
        .accent("C00000")
        .slide(SlideSpec::new("Revenue Deep Dive").bullets(&["ARR grew 18%", "Churn stable at 2%"]))
        .slide(SlideSpec::new("Pipeline Health").bullets(&["Coverage 3.4x", "Zürich office ramping"]).notes("mention hiring"))
        .write_to(&dir.join("q3_review.pptx"))
        .unwrap();
    DeckSpec::new("Product Roadmap")
        .author("Bob")
        .accent("2E7D32")
        .slide(SlideSpec::new("H1 Themes").bullets(&["Search everywhere", "Compose decks fast"]).image())
        .slide(SlideSpec::new("Platform Bets").bullets(&["Rust core", "Native rendering"]))
        .write_to(&dir.join("roadmap.pptx"))
        .unwrap();
}

#[test]
fn index_search_compose_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("decks");
    std::fs::create_dir(&root).unwrap();
    write_fixture_decks(&root);

    // Index.
    let mut lib = Library::open(&tmp.path().join("lib.db")).unwrap();
    lib.add_root(&root).unwrap();
    let mut finished = None;
    lib.scan(&mut |e| {
        if let ScanEvent::Finished { indexed, .. } = e {
            finished = Some(indexed);
        }
    })
    .unwrap();
    assert_eq!(finished, Some(2), "both decks indexed");
    let (decks, slides) = lib.stats().unwrap();
    assert_eq!((decks, slides), (2, 4));

    // Search: prefix + diacritics + snippet mark.
    let hits = lib.search("zurich", &SearchFilters::default()).unwrap();
    assert_eq!(hits.len(), 1, "Zürich slide found via ASCII query");
    assert_eq!(hits[0].slide.title.as_deref(), Some("Pipeline Health"));
    let hits = lib.search("rev", &SearchFilters::default()).unwrap();
    assert!(
        hits.iter().any(|h| h.slide.title.as_deref() == Some("Revenue Deep Dive")),
        "prefix search matches"
    );
    assert!(hits.iter().any(|h| h.snippet.contains("<mark>")), "snippet highlights");

    // Compose one slide from each deck, interleaved order.
    let picks = vec![
        SlidePick { pptx_path: root.join("roadmap.pptx").display().to_string(), slide_index: 1 },
        SlidePick { pptx_path: root.join("q3_review.pptx").display().to_string(), slide_index: 2 },
        SlidePick { pptx_path: root.join("roadmap.pptx").display().to_string(), slide_index: 2 },
    ];
    let out = tmp.path().join("composed.pptx");
    let report = compose(
        &picks,
        &out,
        &ComposeOptions { title: "Board Deck".into(), include_notes: false, fit_mode: None },
    )
    .unwrap();
    assert_eq!(report.slides_written, 3);
    assert_eq!(report.source_decks, 2);

    // Re-open the composed deck: order, content, and style chain intact.
    let pf = PresentationFile::open(&out).unwrap();
    assert_eq!(pf.slide_count(), 3);
    assert_eq!(pf.slide_content(1).unwrap().title.as_deref(), Some("H1 Themes"));
    assert_eq!(pf.slide_content(2).unwrap().title.as_deref(), Some("Pipeline Health"));
    assert_eq!(pf.slide_content(3).unwrap().title.as_deref(), Some("Platform Bets"));
    assert!(pf.slide_content(2).unwrap().notes.is_none(), "notes dropped by default");

    // Both source themes must survive with their distinct accents.
    let theme_parts: Vec<String> = pf
        .package
        .part_names()
        .filter(|n| n.starts_with("ppt/theme/"))
        .map(|s| s.to_string())
        .collect();
    let all_themes: String = theme_parts
        .iter()
        .map(|n| String::from_utf8_lossy(pf.package.part(n).unwrap()).into_owned())
        .collect();
    assert!(all_themes.contains("C00000"), "Q3 review accent preserved");
    assert!(all_themes.contains("2E7D32"), "roadmap accent preserved");

    // Every slide still resolves its full style chain.
    for i in 1..=3 {
        let slide = pf.slide_part(i).unwrap().to_string();
        let layout = pf.layout_of_slide(&slide).unwrap().expect("layout");
        let master = pf.master_of_layout(&layout).unwrap().expect("master");
        let theme = pf.theme_of_master(&master).unwrap().expect("theme");
        assert!(pf.package.has_part(&theme), "theme part exists for slide {i}");
    }

    // Composed slides render.
    let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
    assert!(svg.contains("H1 Themes"));
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));

    // Composed file re-indexes cleanly (it is a fully valid deck).
    let root2 = tmp.path().join("composed_root");
    std::fs::create_dir(&root2).unwrap();
    std::fs::copy(&out, root2.join("board.pptx")).unwrap();
    lib.add_root(&root2).unwrap();
    lib.scan(&mut |_| {}).unwrap();
    let hits = lib.search("platform bets", &SearchFilters::default()).unwrap();
    assert!(hits.iter().any(|h| h.deck.file_name == "board.pptx"));
}

/// Real/large-file corpus, opt-in via SLIDEFLOW_CORPUS=<dir>.
#[test]
fn corpus_index_and_compose() {
    let Ok(dir) = std::env::var("SLIDEFLOW_CORPUS") else {
        eprintln!("SLIDEFLOW_CORPUS not set; skipping");
        return;
    };
    let corpus = Path::new(&dir);
    let tmp = tempfile::tempdir().unwrap();

    let mut lib = Library::open(&tmp.path().join("lib.db")).unwrap();
    lib.add_root(corpus).unwrap();
    let start = std::time::Instant::now();
    let mut indexed = 0;
    lib.scan(&mut |e| {
        if let ScanEvent::Finished { indexed: n, .. } = e {
            indexed = n;
        }
    })
    .unwrap();
    let elapsed = start.elapsed();
    let (decks, slides) = lib.stats().unwrap();
    eprintln!("corpus: {decks} decks / {slides} slides indexed in {elapsed:?}");
    assert!(indexed >= 1);
    assert!(
        elapsed.as_secs() < 60,
        "indexing the corpus must be fast, took {elapsed:?}"
    );

    // Search must be instant-feel.
    let start = std::time::Instant::now();
    let hits = lib.search("revenue", &SearchFilters::default()).unwrap();
    assert!(start.elapsed().as_millis() < 200, "search latency");
    eprintln!("search 'revenue': {} hits in {:?}", hits.len(), start.elapsed());

    // Compose a 10-slide highlight deck from the largest corpus deck + one other.
    let mut decks = lib.decks().unwrap();
    decks.sort_by_key(|d| -d.slide_count);
    if decks.len() >= 2 && decks[0].slide_count >= 10 {
        let picks: Vec<SlidePick> = (1..=8)
            .map(|i| SlidePick { pptx_path: decks[0].path.clone(), slide_index: i * (decks[0].slide_count as usize) / 9 })
            .chain([1usize, 2].into_iter().map(|i| SlidePick { pptx_path: decks[1].path.clone(), slide_index: i }))
            .collect();
        let out = tmp.path().join("highlights.pptx");
        let report = compose(&picks, &out, &ComposeOptions::default()).unwrap();
        assert_eq!(report.slides_written, 10);
        let pf = PresentationFile::open(&out).unwrap();
        assert_eq!(pf.slide_count(), 10);
        for i in 1..=10 {
            let _ = pf.slide_content(i).unwrap();
            let svg = render_slide_svg(&pf, i, &RenderOptions::default()).unwrap();
            assert!(!svg.contains("<script"));
        }
        eprintln!("composed 10-slide highlight deck: {} bytes", std::fs::metadata(&out).unwrap().len());
    }
}
