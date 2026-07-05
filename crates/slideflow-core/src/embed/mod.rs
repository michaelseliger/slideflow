//! Local semantic search: the [`Embedder`] trait, an in-memory [`VectorStore`]
//! for brute-force cosine k-NN, reciprocal-rank fusion for hybrid ranking, and
//! near-duplicate clustering.
//!
//! Everything here is model-free and pure Rust — the trait, vector math, fusion,
//! and duplicate logic are exercised with [`FakeEmbedder`]. The candle-backed
//! real model (`E5Embedder`) lands behind an `embeddings` cargo feature so the
//! default engine build stays lean and needs no ML dependencies.
//!
//! Embedders return **L2-normalized** vectors (cosine == dot product) and bake in
//! their task prefixes: passages and queries are embedded through distinct methods
//! so callers never worry about E5's `passage: ` / `query: ` conventions.

use std::collections::HashMap;

use crate::error::Result;

mod fake;
pub mod store;

#[cfg(feature = "embeddings")]
pub mod e5;

pub use fake::FakeEmbedder;
pub use store::VectorStore;

#[cfg(feature = "embeddings")]
pub use e5::E5Embedder;

/// A text embedder producing fixed-dimension, L2-normalized vectors.
///
/// `Send + Sync` so an `Arc<dyn Embedder>` can live inside the (mutex-guarded)
/// library and be shared across the scan and search paths.
pub trait Embedder: Send + Sync {
    /// Stable identifier persisted alongside every vector (the embeddings row's
    /// `model_id`). Vectors from different embedders never mix.
    fn id(&self) -> &str;
    /// Output dimensionality.
    fn dims(&self) -> usize;
    /// Embed documents (slides). Prepends the passage task prefix internally.
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    /// Embed a search query. Prepends the query task prefix internally.
    fn embed_query(&self, query: &str) -> Result<Vec<f32>>;
}

/// Reciprocal-rank-fusion constant. The classic k=60 damps the influence of any
/// single list's exact positions while still rewarding agreement.
pub const RRF_K: f64 = 60.0;

/// Fuse several ranked id lists by reciprocal rank fusion: an id's score is the
/// sum over the lists it appears in of `1 / (RRF_K + rank)` (rank is 1-based).
/// Returns `(id, score)` best-first; ties broken by lower id for determinism.
///
/// An id present in more lists — or higher within them — outranks one seen in a
/// single list, which is exactly the hybrid-search property we want: agreement
/// between lexical and semantic retrieval wins.
pub fn rrf_fuse(lists: &[&[i64]]) -> Vec<(i64, f64)> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for list in lists {
        for (rank0, &id) in list.iter().enumerate() {
            *scores.entry(id).or_insert(0.0) += 1.0 / (RRF_K + (rank0 as f64 + 1.0));
        }
    }
    let mut fused: Vec<(i64, f64)> = scores.into_iter().collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0))
    });
    fused
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agreement_outranks_single_list() {
        // 7 is in both lists (mid rank in each); 1 tops the lexical list only.
        let lexical = [1i64, 2, 3, 7, 4];
        let semantic = [5i64, 6, 7, 8, 9];
        let fused = rrf_fuse(&[&lexical, &semantic]);
        let order: Vec<i64> = fused.iter().map(|(id, _)| *id).collect();
        assert_eq!(order[0], 7, "the doc in both lists wins");
        // A semantic-only doc (5) still surfaces in the fused output.
        assert!(order.contains(&5));
    }

    #[test]
    fn single_list_preserves_order() {
        let only = [10i64, 20, 30];
        let fused = rrf_fuse(&[&only]);
        assert_eq!(fused.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![10, 20, 30]);
    }
}
