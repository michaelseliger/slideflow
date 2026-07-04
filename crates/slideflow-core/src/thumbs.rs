//! Content-addressed keys for the on-disk slide-preview cache.
//!
//! The cache filename is derived from the *deck's identity* (its path + content
//! hash), the slide index, the render-format version, and the quality tier —
//! never from the SQLite slide rowid. Rowids are recycled after `DELETE` (the
//! `slides`/`decks` tables use plain `INTEGER PRIMARY KEY`), so a filename keyed
//! on the id would let a brand-new slide inherit a removed slide's cached SVG.
//! Content-addressing makes that collision impossible and needs no eviction
//! bookkeeping: a changed or removed deck simply stops matching its old files,
//! and [`sweep_thumbs`] reclaims the orphans.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::render::RENDER_VERSION;

/// Quality tier of a cached preview. The grid uses [`ThumbTier::Thumb`]
/// (small, images heavily downscaled); the peek modal / inspector use
/// [`ThumbTier::Full`] (larger, crisper). Each tier is a distinct cache file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbTier {
    Thumb,
    Full,
}

impl ThumbTier {
    pub fn as_str(self) -> &'static str {
        match self {
            ThumbTier::Thumb => "thumb",
            ThumbTier::Full => "full",
        }
    }

    /// Parse the tier string the frontend sends. Anything unrecognized (or
    /// absent) falls back to the cheap grid tier.
    pub fn parse(s: &str) -> Self {
        match s {
            "full" => ThumbTier::Full,
            _ => ThumbTier::Thumb,
        }
    }
}

/// First 16 hex chars (64 bits) of the SHA-256 of `s` — collision-proof at any
/// realistic library size, and safe as a filename component.
fn hash16(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The cache filename for one slide's preview at a given tier.
///
/// `content_hash` is the deck's stored hash (see `index::content_hash`). Note it
/// hashes only `(mtime, size)`, so two distinct files *could* share one — the
/// deck-path hash disambiguates them. Changing the file changes its hash, which
/// changes the filename, which invalidates the cache for free.
pub fn thumb_file_name(
    deck_path: &str,
    content_hash: &str,
    slide_index: usize,
    tier: ThumbTier,
) -> String {
    let ph = hash16(deck_path);
    let ch: String = content_hash.chars().take(16).collect();
    format!("t-{ph}-{ch}-{slide_index}-r{RENDER_VERSION}-{}.svg", tier.as_str())
}

/// Delete every cache file in `dir` that no current deck can legitimately claim:
/// legacy `<rowid>.svg` files from before content-addressing, thumbs of a stale
/// render version, and orphans of decks (or deck versions) no longer indexed.
/// `valid` holds `(deck_path, content_hash)` for every deck currently in the
/// library. Only regular files directly in `dir` are considered; subdirectories
/// are left alone. Best-effort — I/O errors are ignored. Returns the count
/// removed.
///
/// This is also the migration path: a fresh upgrade sweeps away all pre-existing
/// `<id>.svg` files on first run, since none match the `t-…` scheme.
pub fn sweep_thumbs(dir: &Path, valid: &HashSet<(String, String)>) -> usize {
    let valid_prefixes: HashSet<String> = valid
        .iter()
        .map(|(path, chash)| {
            let ph = hash16(path);
            let ch: String = chash.chars().take(16).collect();
            format!("t-{ph}-{ch}-")
        })
        .collect();
    let version_tag = format!("-r{RENDER_VERSION}-");

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let keep = name.starts_with("t-")
            && name.contains(&version_tag)
            && valid_prefixes.iter().any(|p| name.starts_with(p));
        if !keep && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn key_is_stable_and_uses_render_version() {
        let a = thumb_file_name("/decks/a.pptx", H, 3, ThumbTier::Thumb);
        assert_eq!(a, thumb_file_name("/decks/a.pptx", H, 3, ThumbTier::Thumb));
        assert!(a.starts_with("t-"));
        assert!(a.contains(&format!("-r{RENDER_VERSION}-")));
        assert!(a.ends_with("-thumb.svg"));
    }

    #[test]
    fn key_varies_on_every_component() {
        let base = thumb_file_name("/decks/a.pptx", H, 3, ThumbTier::Thumb);
        // different deck path (same content hash — the (mtime,size) collision case)
        assert_ne!(base, thumb_file_name("/decks/b.pptx", H, 3, ThumbTier::Thumb));
        // different content hash (file changed)
        let h2 = "ffffffffffffffff0000000000000000ffffffffffffffff0000000000000000";
        assert_ne!(base, thumb_file_name("/decks/a.pptx", h2, 3, ThumbTier::Thumb));
        // different slide index
        assert_ne!(base, thumb_file_name("/decks/a.pptx", H, 4, ThumbTier::Thumb));
        // different tier
        assert_ne!(base, thumb_file_name("/decks/a.pptx", H, 3, ThumbTier::Full));
    }

    #[test]
    fn sweep_removes_legacy_and_orphans_keeps_valid_and_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // A valid deck's thumb (both tiers) — must survive.
        let keep_thumb = thumb_file_name("/decks/a.pptx", H, 0, ThumbTier::Thumb);
        let keep_full = thumb_file_name("/decks/a.pptx", H, 0, ThumbTier::Full);
        fs::write(root.join(&keep_thumb), b"x").unwrap();
        fs::write(root.join(&keep_full), b"x").unwrap();

        // Legacy pre-content-addressing file — must go.
        fs::write(root.join("42.svg"), b"x").unwrap();
        // Orphan of a deck no longer in the library — must go.
        let orphan = thumb_file_name("/decks/gone.pptx", H, 0, ThumbTier::Thumb);
        fs::write(root.join(&orphan), b"x").unwrap();
        // Stale content hash for a valid deck — must go.
        let stale = thumb_file_name("/decks/a.pptx", "deadbeefdeadbeef", 0, ThumbTier::Thumb);
        fs::write(root.join(&stale), b"x").unwrap();

        // A subdirectory — must be left untouched.
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("nested.svg"), b"x").unwrap();

        let mut valid = HashSet::new();
        valid.insert(("/decks/a.pptx".to_string(), H.to_string()));

        let removed = sweep_thumbs(root, &valid);
        assert_eq!(removed, 3);
        assert!(root.join(&keep_thumb).exists());
        assert!(root.join(&keep_full).exists());
        assert!(!root.join("42.svg").exists());
        assert!(!root.join(&orphan).exists());
        assert!(!root.join(&stale).exists());
        assert!(root.join("sub").join("nested.svg").exists());
    }
}
