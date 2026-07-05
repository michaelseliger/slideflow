//! End-to-end test for the real candle-backed E5 embedder. Compiled only with
//! `--features embeddings` and `#[ignore]`d unless `SLIDEFLOW_E5_DIR` points at a
//! downloaded `multilingual-e5-small` directory (model.safetensors, tokenizer.json,
//! config.json). Run it with:
//!
//! ```bash
//! SLIDEFLOW_E5_DIR=~/.cache/slideflow-dev/e5 \
//!   cargo test -p slideflow-core --features embeddings --test e5_embeddings -- --ignored
//! ```
#![cfg(feature = "embeddings")]

use slideflow_core::embed::{E5Embedder, Embedder};

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[test]
#[ignore = "requires SLIDEFLOW_E5_DIR pointing at a downloaded multilingual-e5-small dir"]
fn cross_lingual_retrieval_ranks_translation_first() {
    let dir = std::env::var("SLIDEFLOW_E5_DIR")
        .expect("set SLIDEFLOW_E5_DIR to the downloaded model directory");
    let e = E5Embedder::load(std::path::Path::new(&dir)).expect("load model");
    assert_eq!(e.dims(), 384, "multilingual-e5-small is 384-dimensional");

    // A German "customer churn" passage and an unrelated German passage. An
    // English query must rank the churn passage first — proving cross-lingual
    // (DE/EN) semantic retrieval works.
    let passages = vec![
        "Die Kundenabwanderung ist im dritten Quartal deutlich gesunken.".to_string(),
        "Unser Koch bereitet ein köstliches Pastagericht mit frischen Kräutern zu.".to_string(),
    ];
    let vecs = e.embed_passages(&passages).expect("embed passages");
    assert_eq!(vecs.len(), 2);
    // Vectors are unit length.
    for v in &vecs {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "vector not normalized: {norm}");
    }

    let q = e.embed_query("customer churn rate").expect("embed query");
    let sim_churn = cosine(&q, &vecs[0]);
    let sim_food = cosine(&q, &vecs[1]);
    assert!(
        sim_churn > sim_food,
        "German churn passage must outrank the unrelated one: {sim_churn} vs {sim_food}"
    );
}
