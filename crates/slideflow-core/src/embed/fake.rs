//! A deterministic, dependency-free [`Embedder`] for tests and for exercising the
//! whole pipeline (schema, hashing, backfill, fusion, duplicates) without loading
//! a real model. Each text maps to a fixed pseudo-random unit vector seeded by its
//! sha256, so identical text always embeds identically (queries too — a query
//! equal to a passage yields cosine 1.0). Call counts are exposed so tests can
//! assert that only missing texts are embedded and that lexical search never
//! embeds a query.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use sha2::{Digest, Sha256};

use super::store::l2_normalize;
use super::Embedder;
use crate::error::{Error, Result};

pub struct FakeEmbedder {
    id: String,
    dims: usize,
    passages: AtomicUsize,
    queries: AtomicUsize,
    /// When set, `embed_passages` returns an error instead of vectors — used to
    /// exercise the "embedding failure never aborts the scan" contract.
    fail_passages: AtomicBool,
}

impl FakeEmbedder {
    pub fn new(dims: usize) -> Self {
        FakeEmbedder {
            id: "fake-v1".to_string(),
            dims,
            passages: AtomicUsize::new(0),
            queries: AtomicUsize::new(0),
            fail_passages: AtomicBool::new(false),
        }
    }

    pub fn with_id(id: impl Into<String>, dims: usize) -> Self {
        FakeEmbedder { id: id.into(), ..Self::new(dims) }
    }

    /// Make every subsequent `embed_passages` call fail (simulates a
    /// model/tokenizer error mid-scan). Toggleable so a test can fail the inline
    /// scan embed and then let the backfill succeed.
    pub fn set_fail_passages(&self, fail: bool) {
        self.fail_passages.store(fail, Ordering::Relaxed);
    }

    /// Total number of passage texts embedded across all `embed_passages` calls.
    pub fn passage_count(&self) -> usize {
        self.passages.load(Ordering::Relaxed)
    }

    /// Total number of `embed_query` calls.
    pub fn query_count(&self) -> usize {
        self.queries.load(Ordering::Relaxed)
    }
}

impl Embedder for FakeEmbedder {
    fn id(&self) -> &str {
        &self.id
    }
    fn dims(&self) -> usize {
        self.dims
    }
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if self.fail_passages.load(Ordering::Relaxed) {
            return Err(Error::Embedding("fake embedder: forced passage failure".into()));
        }
        self.passages.fetch_add(texts.len(), Ordering::Relaxed);
        Ok(texts.iter().map(|t| fake_vec(t, self.dims)).collect())
    }
    fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        self.queries.fetch_add(1, Ordering::Relaxed);
        Ok(fake_vec(query, self.dims))
    }
}

/// Deterministic unit vector for `text`: seed a small xorshift PRNG from the
/// text's sha256 and draw `dims` values in [-1, 1], then L2-normalize.
fn fake_vec(text: &str, dims: usize) -> Vec<f32> {
    let digest = Sha256::digest(text.as_bytes());
    let mut state = u64::from_le_bytes(digest[..8].try_into().unwrap()) | 1;
    let mut v = Vec::with_capacity(dims);
    for _ in 0..dims {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let r = state.wrapping_mul(0x2545F4914F6CDD1D);
        // Map the top 24 bits to [-1, 1].
        let unit = (r >> 40) as f32 / ((1u64 << 24) as f32);
        v.push(unit * 2.0 - 1.0);
    }
    l2_normalize(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_normalized() {
        let e = FakeEmbedder::new(16);
        let a = e.embed_passages(&["hello".to_string()]).unwrap();
        let b = e.embed_passages(&["hello".to_string()]).unwrap();
        assert_eq!(a, b, "same text embeds identically");
        let norm: f32 = a[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "unit length");
        // Query of the same text matches its passage (cosine 1.0).
        let q = e.embed_query("hello").unwrap();
        let cos: f32 = q.iter().zip(&a[0]).map(|(x, y)| x * y).sum();
        assert!((cos - 1.0).abs() < 1e-5);
    }

    #[test]
    fn counts_calls() {
        let e = FakeEmbedder::new(8);
        e.embed_passages(&["a".into(), "b".into()]).unwrap();
        e.embed_query("q").unwrap();
        assert_eq!(e.passage_count(), 2);
        assert_eq!(e.query_count(), 1);
    }
}
