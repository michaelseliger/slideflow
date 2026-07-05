//! Content-addressed cache keys for the desktop "drag a slide out" scratch
//! files (WS-G).
//!
//! When a slide is dragged out of the app (or saved via the context menu) the
//! host writes two ephemeral files under `app_cache/dragout`: a single-slide
//! `.pptx` (composed with full formatting) and a small PNG drag preview. Those
//! are regenerated only on a cache miss.
//!
//! The key bakes in the source deck's modification time, so staleness
//! self-invalidates: edit the deck and the key changes, so the previous files
//! no longer match and are rebuilt — exactly the philosophy of [`crate::thumbs`],
//! but keyed on `(deck_path, slide_index, mtime)` rather than the stored content
//! hash, because the host has the file's mtime cheaply to hand and never needs a
//! library lookup on the drag path. Because a present-and-matching file is
//! always fresh, the cache check is just "do both files exist"; the whole
//! `dragout` dir is wiped on app startup so nothing accumulates across runs.

use sha2::{Digest, Sha256};

/// First 16 hex chars (64 bits) of the SHA-256 of `s` — collision-proof at any
/// realistic library size, and safe as a filename component.
fn hash16(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// The content-addressed cache key for one slide's drag-out scratch files.
///
/// `deck_mtime_secs` is the source deck's modification time in whole seconds
/// since the Unix epoch. The key varies on every input, so a changed deck (new
/// mtime) yields a new key and the stale files simply stop matching. The host
/// uses this as the tail of the on-disk file stem; the deck path is hashed (not
/// embedded raw) so it stays a single safe path component.
pub fn cache_key(deck_path: &str, slide_index: usize, deck_mtime_secs: u64) -> String {
    let ph = hash16(deck_path);
    format!("{ph}-{slide_index}-m{deck_mtime_secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_stable_for_identical_inputs() {
        let a = cache_key("/decks/a.pptx", 3, 1_700_000_000);
        let b = cache_key("/decks/a.pptx", 3, 1_700_000_000);
        assert_eq!(a, b);
    }

    #[test]
    fn key_varies_on_every_component() {
        let base = cache_key("/decks/a.pptx", 3, 1_700_000_000);
        // different deck path
        assert_ne!(base, cache_key("/decks/b.pptx", 3, 1_700_000_000));
        // different slide index
        assert_ne!(base, cache_key("/decks/a.pptx", 4, 1_700_000_000));
        // different mtime — this is what makes an edited deck self-invalidate
        assert_ne!(base, cache_key("/decks/a.pptx", 3, 1_700_000_001));
    }

    #[test]
    fn key_is_a_single_safe_path_component() {
        // No separators / colons leak in from the (hashed) deck path.
        let k = cache_key("/decks/sub dir/a: weird\\name.pptx", 12, 42);
        assert!(!k.contains('/') && !k.contains('\\') && !k.contains(':'));
        assert!(k.ends_with("-12-m42"));
    }
}
