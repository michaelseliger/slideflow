//! In-memory vector store: brute-force cosine k-NN over slide embeddings.
//!
//! Vectors are L2-normalized at write time, so cosine similarity is a plain dot
//! product. The store keeps one row per *slide* (slides sharing a text hash share
//! an identical row) in a contiguous `f32` matrix; searches are rayon-parallel.
//! There is deliberately no ANN index — a few thousand-to-tens-of-thousands of
//! 384-d vectors brute-force in well under a frame, and correctness is trivial.

use std::collections::HashMap;

use rayon::prelude::*;

/// Encode an f32 vector to a little-endian byte blob for SQLite storage.
pub fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

/// Decode a little-endian f32 blob written by [`vec_to_blob`]. Trailing bytes
/// that don't form a whole f32 are ignored (defensive; never happens on our
/// own writes).
pub fn blob_to_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// L2-normalize a vector in place. A zero vector is left untouched.
pub fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// A loaded set of slide embeddings for one model.
pub struct VectorStore {
    model_id: String,
    dims: usize,
    /// Row i → slide rowid.
    slide_ids: Vec<i64>,
    /// Row i → that slide's text hash (used to exclude same-text twins).
    text_hashes: Vec<String>,
    /// `slide_ids.len() * dims`, row-major, each row L2-normalized.
    matrix: Vec<f32>,
}

impl VectorStore {
    /// Build a store from parallel per-slide arrays. Rows whose vector length
    /// doesn't match `dims` are skipped (defensive against a stale/mismatched
    /// embedding row); vectors are re-normalized so dot products are cosines.
    pub fn new(
        model_id: impl Into<String>,
        dims: usize,
        rows: Vec<(i64, String, Vec<f32>)>,
    ) -> Self {
        let mut slide_ids = Vec::with_capacity(rows.len());
        let mut text_hashes = Vec::with_capacity(rows.len());
        let mut matrix = Vec::with_capacity(rows.len() * dims);
        for (id, th, mut vec) in rows {
            if vec.len() != dims {
                continue;
            }
            l2_normalize(&mut vec);
            slide_ids.push(id);
            text_hashes.push(th);
            matrix.extend_from_slice(&vec);
        }
        VectorStore { model_id: model_id.into(), dims, slide_ids, text_hashes, matrix }
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }
    pub fn dims(&self) -> usize {
        self.dims
    }
    pub fn len(&self) -> usize {
        self.slide_ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.slide_ids.is_empty()
    }
    pub fn slide_ids(&self) -> &[i64] {
        &self.slide_ids
    }

    pub fn text_hash_at(&self, row: usize) -> &str {
        &self.text_hashes[row]
    }

    /// The vector for row `i`.
    pub fn row(&self, i: usize) -> &[f32] {
        &self.matrix[i * self.dims..(i + 1) * self.dims]
    }

    /// First row whose slide id matches, if the slide is embedded.
    pub fn row_of_slide(&self, slide_id: i64) -> Option<usize> {
        self.slide_ids.iter().position(|&id| id == slide_id)
    }

    /// Top-`k` rows by cosine to `query` (a normalized vector), skipping rows for
    /// which `exclude(row)` is true. Returns `(row_index, score)` best-first; ties
    /// broken by lower row index for determinism.
    pub fn top_k<F>(&self, query: &[f32], k: usize, exclude: F) -> Vec<(usize, f32)>
    where
        F: Fn(usize) -> bool + Sync,
    {
        if k == 0 || self.is_empty() || query.len() != self.dims {
            return Vec::new();
        }
        let scores: Vec<f32> = (0..self.len())
            .into_par_iter()
            .map(|i| {
                if exclude(i) {
                    f32::NEG_INFINITY
                } else {
                    dot(query, self.row(i))
                }
            })
            .collect();
        let mut idx: Vec<usize> = (0..self.len()).filter(|&i| scores[i].is_finite()).collect();
        idx.sort_unstable_by(|&a, &b| {
            scores[b].partial_cmp(&scores[a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b))
        });
        idx.truncate(k);
        idx.into_iter().map(|i| (i, scores[i])).collect()
    }

    /// Top-`k` neighbors of row `i` (excluding itself).
    fn top_k_for_row(&self, i: usize, k: usize) -> Vec<(usize, f32)> {
        self.top_k(self.row(i), k, |j| j == i)
    }

    /// Cluster slides into near-duplicate groups: union-find over every edge
    /// `(i, j)` where `j` is among row `i`'s top-`top_n` neighbors with cosine
    /// `>= threshold`. Returns groups of slide ids (each of size ≥ 2), largest
    /// first. Neighbor search is rayon-parallel.
    pub fn near_dup_clusters(&self, threshold: f32, top_n: usize) -> Vec<Vec<i64>> {
        let n = self.len();
        if n < 2 {
            return Vec::new();
        }
        let edges: Vec<(usize, usize)> = (0..n)
            .into_par_iter()
            .flat_map_iter(|i| {
                self.top_k_for_row(i, top_n)
                    .into_iter()
                    .filter(move |&(_, s)| s >= threshold)
                    .map(move |(j, _)| if i < j { (i, j) } else { (j, i) })
                    .collect::<Vec<_>>()
                    .into_iter()
            })
            .collect();

        let mut uf = UnionFind::new(n);
        for (a, b) in edges {
            uf.union(a, b);
        }
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..n {
            groups.entry(uf.find(i)).or_default().push(i);
        }
        let mut out: Vec<Vec<i64>> = groups
            .into_values()
            .filter(|g| g.len() >= 2)
            .map(|mut g| {
                g.sort_unstable();
                g.into_iter().map(|i| self.slide_ids[i]).collect()
            })
            .collect();
        // Largest groups first; ties broken by the group's smallest slide id.
        out.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
        out
    }

    /// Mean cosine of every member of a slide-id group to the group's first
    /// member — a cheap "how alike" score for a near-duplicate cluster. `None`
    /// if fewer than two members are present in the store.
    pub fn group_cohesion(&self, slide_ids: &[i64]) -> Option<f32> {
        let rows: Vec<usize> = slide_ids.iter().filter_map(|&id| self.row_of_slide(id)).collect();
        if rows.len() < 2 {
            return None;
        }
        let anchor = self.row(rows[0]);
        let sum: f32 = rows[1..].iter().map(|&r| dot(anchor, self.row(r))).sum();
        Some(sum / (rows.len() - 1) as f32)
    }
}

#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Weighted-quick-union with path compression.
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind { parent: (0..n).collect(), rank: vec![0; n] }
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        match self.rank[ra].cmp(&self.rank[rb]) {
            std::cmp::Ordering::Less => self.parent[ra] = rb,
            std::cmp::Ordering::Greater => self.parent[rb] = ra,
            std::cmp::Ordering::Equal => {
                self.parent[rb] = ra;
                self.rank[ra] += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(rows: Vec<(i64, &str, Vec<f32>)>) -> VectorStore {
        VectorStore::new(
            "test",
            2,
            rows.into_iter().map(|(id, th, v)| (id, th.to_string(), v)).collect(),
        )
    }

    #[test]
    fn blob_roundtrip() {
        let v = vec![0.5f32, -0.25, 1.0, 0.0];
        assert_eq!(blob_to_vec(&vec_to_blob(&v)), v);
    }

    #[test]
    fn top_k_orders_by_cosine_and_excludes() {
        // Unit vectors around a circle; query points along +x.
        let s = store(vec![
            (10, "a", vec![1.0, 0.0]),   // cos 1.0
            (20, "b", vec![0.0, 1.0]),   // cos 0.0
            (30, "c", vec![-1.0, 0.0]),  // cos -1.0
            (40, "d", vec![0.7, 0.7]),   // cos ~0.707
        ]);
        let q = vec![1.0, 0.0];
        let hits = s.top_k(&q, 3, |_| false);
        let ids: Vec<i64> = hits.iter().map(|(i, _)| s.slide_ids()[*i]).collect();
        assert_eq!(ids, vec![10, 40, 20], "best cosine first");
        assert!((hits[0].1 - 1.0).abs() < 1e-6);

        // Exclusion by row.
        let hits = s.top_k(&q, 3, |i| s.slide_ids()[i] == 10);
        let ids: Vec<i64> = hits.iter().map(|(i, _)| s.slide_ids()[*i]).collect();
        assert_eq!(ids, vec![40, 20, 30]);
    }

    #[test]
    fn near_dup_union_find_merges_transitively() {
        // Two tight clusters plus one loner. Cluster 1: a≈a'; cluster 2: b≈b'≈b''.
        let s = store(vec![
            (1, "h1", vec![1.0, 0.02]),
            (2, "h2", vec![1.0, 0.03]), // ~a
            (3, "h3", vec![0.0, 1.0]),
            (4, "h4", vec![0.02, 1.0]), // ~b
            (5, "h5", vec![0.03, 1.0]), // ~b (transitively linked to 3 via 4)
            (6, "h6", vec![0.7, -0.7]), // loner
        ]);
        let mut clusters = s.near_dup_clusters(0.92, 10);
        // Normalize for comparison.
        for c in &mut clusters {
            c.sort_unstable();
        }
        clusters.sort_by_key(|c| c[0]);
        assert_eq!(clusters, vec![vec![1, 2], vec![3, 4, 5]]);
    }

    #[test]
    fn group_cohesion_scores_similarity() {
        let s = store(vec![(1, "h1", vec![1.0, 0.0]), (2, "h2", vec![1.0, 0.0])]);
        let c = s.group_cohesion(&[1, 2]).unwrap();
        assert!((c - 1.0).abs() < 1e-6);
        assert_eq!(s.group_cohesion(&[1]), None);
    }
}
