//! App-local fonts — everything the pure engine must not do: read the fonts
//! directory, harvest embedded faces to disk, fetch fonts over the network, and
//! keep the export `fontdb` + the render `AppFontSet` in sync as fonts come and
//! go.
//!
//! Fonts live under `<app_data>/fonts/`, in three source subdirs so a family's
//! provenance is obvious and removable:
//! - `harvested/` — copied out of decks that EMBED a font whose OS/2 `fsType`
//!   permits reuse ([`slideflow_core::pptx::embedded_fonts::is_harvestable`]), so
//!   every deck naming that family benefits, not just the embedding one.
//! - `user/` — the user's own licensed fonts, added via the Settings picker (or
//!   dropped into the folder by hand; rescanned on demand).
//! - `downloaded/` — families the curated resolver can legally fetch (Google
//!   Fonts OFL; Microsoft's free Aptos zip), each pulled only on explicit
//!   consent.
//!
//! The engine stays network- and filesystem-side-effect-free: this module builds
//! a [`slideflow_core::fonts::AppFontSet`] (injected into renders) and a
//! `fontdb::Database` (system + bundled substitutes + app fonts, for export),
//! and rebuilds both whenever the set changes — then wipes the preview cache and
//! emits `fonts:changed` so the UI re-renders.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use slideflow_core::export::{fontdb, system_fonts};
use slideflow_core::fonts::{AppFontFace, AppFontSet};
use slideflow_core::pptx::embedded_fonts::is_harvestable;
use slideflow_core::pptx::PresentationFile;

use crate::commands::AppState;

/// The three source subdirectories of `<app_data>/fonts/`, in label-precedence
/// order (the first that provides a family wins its source label).
const SUBDIRS: [(&str, FontSourceKind); 3] = [
    ("user", FontSourceKind::User),
    ("downloaded", FontSourceKind::Downloaded),
    ("harvested", FontSourceKind::Harvested),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FontSourceKind {
    Harvested,
    User,
    Downloaded,
}

impl FontSourceKind {
    fn label(self) -> &'static str {
        match self {
            FontSourceKind::Harvested => "harvested",
            FontSourceKind::User => "user",
            FontSourceKind::Downloaded => "downloaded",
        }
    }
}

/// An app-installed family: where it lives and the files backing it (so a remove
/// can delete exactly those). A family present in more than one subdir keeps the
/// highest-precedence source label but tracks every file for removal.
#[derive(Debug, Clone)]
struct Installed {
    family: String,
    source: FontSourceKind,
    files: Vec<PathBuf>,
}

struct FontsInner {
    /// App faces only — injected into renders for `@font-face` embedding.
    app_set: Arc<AppFontSet>,
    /// family(lowercased) → where it came from + its files.
    installed: HashMap<String, Installed>,
    /// system + bundled substitutes + app faces, for the export path. Rebuilt
    /// lazily (the system-font scan is slow) and invalidated on any change.
    db: Option<Arc<fontdb::Database>>,
    /// Monotonic font-set version, bumped on every [`FontsState::rebuild`]. Lets
    /// [`FontsState::db`] detect a rebuild that raced its (lock-free) build and
    /// refuse to cache a now-stale db, and lets a slow render skip caching an SVG
    /// whose fonts changed underneath it (see `get_slide_preview`).
    generation: u64,
}

/// Tauri-managed app-font state. Owns the fonts dir, the rebuildable render/
/// export databases, and the single-download guard.
pub struct FontsState {
    /// `<app_data>/fonts`.
    pub dir: PathBuf,
    inner: RwLock<FontsInner>,
    /// Re-entrancy guard + liveness flag for the download thread.
    downloading: AtomicBool,
    download_cancel: AtomicBool,
    /// Last download failure, surfaced via the returned list / events.
    last_error: Mutex<Option<String>>,
}

impl FontsState {
    /// Build from the on-disk fonts dir. Cheap — it parses only the (small) app
    /// font files, never the system fonts (that scan is deferred to [`Self::db`],
    /// which callers hit from a blocking thread).
    pub fn new(dir: PathBuf) -> Self {
        let (app_set, installed) = scan_dir(&dir);
        FontsState {
            dir,
            inner: RwLock::new(FontsInner {
                app_set: Arc::new(app_set),
                installed,
                db: None,
                generation: 0,
            }),
            downloading: AtomicBool::new(false),
            download_cancel: AtomicBool::new(false),
            last_error: Mutex::new(None),
        }
    }

    /// The app fonts to inject into a render (`RenderOptions::app_fonts`),
    /// together with the generation they belong to — snapshotted under ONE lock so
    /// the two can't diverge. A caller renders with these faces, then compares
    /// [`Self::generation`] afterwards to tell whether the font set changed
    /// underneath it (and skip caching a stale result).
    pub fn app_set_with_generation(&self) -> (Arc<AppFontSet>, u64) {
        let inner = self.inner.read().unwrap_or_else(|p| p.into_inner());
        (inner.app_set.clone(), inner.generation)
    }

    /// The export font database: system fonts + bundled substitutes + app fonts.
    /// Built lazily on first use (the system scan is slow — call from a blocking
    /// thread), cached until the next font-set change invalidates it.
    ///
    /// The build runs lock-free (the first call includes the 100–300 ms system
    /// scan), so a [`Self::rebuild`] can land while it runs. To avoid caching a db
    /// that's already stale — which would serve old fonts until the *next* font
    /// change — we snapshot the app faces AND their generation together, then cache
    /// only if the generation still matches under the write lock. On a race, the
    /// freshly built db (correct for the snapshot we took) is returned uncached, so
    /// the next caller rebuilds against the new generation.
    pub fn db(&self) -> Arc<fontdb::Database> {
        // Snapshot the app faces and the generation they belong to together, or
        // return an already-cached db.
        let (app_set, generation) = {
            let inner = self.inner.read().unwrap_or_else(|p| p.into_inner());
            if let Some(db) = &inner.db {
                return db.clone();
            }
            (inner.app_set.clone(), inner.generation)
        };
        // Deep-clone the shared system+bundled database (its OnceLock is already
        // warm after the first export) and add the app faces — lock-free.
        let mut db = (*system_fonts()).clone();
        app_set.register(&mut db);
        let arc = Arc::new(db);

        let mut inner = self.inner.write().unwrap_or_else(|p| p.into_inner());
        if inner.generation != generation {
            // A rebuild raced our build; `arc` is valid for the fonts we snapshotted
            // but not the current set. Hand it to this caller uncached so the cache
            // isn't poisoned; the next db() call rebuilds against the new generation.
            return arc;
        }
        // Another thread may have cached an identical-generation db while we built —
        // prefer the existing one so all callers share a single Arc.
        inner.db.get_or_insert(arc).clone()
    }

    /// The current font-set generation (bumped on every [`Self::rebuild`]). A
    /// caller that snapshots it before a slow render can tell whether the fonts
    /// changed underneath it and skip caching a now-stale result.
    pub fn generation(&self) -> u64 {
        self.inner.read().unwrap_or_else(|p| p.into_inner()).generation
    }

    /// Re-scan the fonts dir and rebuild the render set; the export db is
    /// invalidated and the generation bumped so the next [`Self::db`] rebuilds it
    /// with the new faces and any racing in-flight build refuses to cache.
    fn rebuild(&self) {
        let (app_set, installed) = scan_dir(&self.dir);
        let mut inner = self.inner.write().unwrap_or_else(|p| p.into_inner());
        inner.app_set = Arc::new(app_set);
        inner.installed = installed;
        inner.db = None;
        inner.generation = inner.generation.wrapping_add(1);
    }

    fn installed_family(&self, family: &str) -> Option<Installed> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .installed
            .get(&family.to_ascii_lowercase())
            .cloned()
    }

    fn set_error(&self, msg: Option<String>) {
        *self.last_error.lock().unwrap_or_else(|p| p.into_inner()) = msg;
    }
}

// ---------------------------------------------------------------------------
// Directory scan → AppFontSet + installed map
// ---------------------------------------------------------------------------

/// Whether `bytes` starts with a recognized sfnt magic (raw TTF/OTF/TTC). A
/// local mirror of the engine's private check — the host validates copied /
/// downloaded fonts the same way.
fn is_sfnt(bytes: &[u8]) -> bool {
    matches!(bytes.get(..4), Some([0x00, 0x01, 0x00, 0x00]))
        || bytes.starts_with(b"OTTO")
        || bytes.starts_with(b"true")
        || bytes.starts_with(b"ttcf")
}

/// A file's font family + bold/italic, read via a throwaway `fontdb` (the same
/// parser the export path uses, so what we record here matches what resolves at
/// rasterization time). `None` when the bytes aren't a parseable font.
fn face_identity(bytes: &[u8]) -> Option<(String, bool, bool)> {
    let mut db = fontdb::Database::new();
    db.load_font_data(bytes.to_vec());
    let face = db.faces().next()?;
    let family = face.families.first().map(|(f, _)| f.clone())?;
    let bold = face.weight.0 >= 600;
    let italic = face.style != fontdb::Style::Normal;
    Some((family, bold, italic))
}

/// Scan the three source subdirs into an [`AppFontSet`] and an installed-family
/// map. Faces are keyed by their real `name`-table family; a family found in
/// several subdirs keeps the first (highest-precedence) source label but
/// accumulates every file so a remove deletes them all.
fn scan_dir(dir: &Path) -> (AppFontSet, HashMap<String, Installed>) {
    let mut faces: Vec<AppFontFace> = Vec::new();
    let mut installed: HashMap<String, Installed> = HashMap::new();

    for (sub, kind) in SUBDIRS {
        let subdir = dir.join(sub);
        let Ok(entries) = std::fs::read_dir(&subdir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_font_file(&path) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if !is_sfnt(&bytes) {
                continue;
            }
            let Some((family, bold, italic)) = face_identity(&bytes) else { continue };
            let key = family.to_ascii_lowercase();

            faces.push(AppFontFace { family: family.clone(), bold, italic, bytes: Arc::new(bytes) });

            installed
                .entry(key)
                .and_modify(|e| e.files.push(path.clone()))
                .or_insert_with(|| Installed { family, source: kind, files: vec![path.clone()] });
        }
    }

    (AppFontSet::new(faces), installed)
}

/// Whether `path` is a `.ttf`/`.otf` file (case-insensitive).
fn is_font_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("ttf") | Some("otf")
    )
}

// ---------------------------------------------------------------------------
// Curated download resolver (host-side only — core never touches the network)
// ---------------------------------------------------------------------------

/// How a curated family is fetched.
enum Fetch {
    /// Direct font files (Google Fonts OFL raw content). `(url, output name)`.
    Files(&'static [(&'static str, &'static str)]),
    /// A zip to unpack — every `.ttf`/`.otf` entry is kept. `url`.
    Zip(&'static str),
}

struct Curated {
    /// The family name as decks reference it (and the font's own `name` table).
    family: &'static str,
    /// Human-readable provenance shown in the consent dialog.
    source: &'static str,
    fetch: Fetch,
}

/// The curated catalog: families we can legally fetch from official sources.
/// Google Fonts entries pin raw paths against `google/fonts@main`; the `[`/`]`
/// in variable-font file names are percent-encoded (`%5B`/`%5D`).
/// Google Fonts entries use pinned raw paths against `google/fonts@main`
/// (verified variable-font file names); Aptos is Microsoft's official free zip.
const CATALOG: &[Curated] = &[
    Curated {
        family: "Karla",
        source: "Google Fonts (OFL) · github.com/google/fonts",
        fetch: Fetch::Files(&[
            (
                "https://raw.githubusercontent.com/google/fonts/main/ofl/karla/Karla%5Bwght%5D.ttf",
                "Karla.ttf",
            ),
            (
                "https://raw.githubusercontent.com/google/fonts/main/ofl/karla/Karla-Italic%5Bwght%5D.ttf",
                "Karla-Italic.ttf",
            ),
        ]),
    },
    Curated {
        family: "Montserrat",
        source: "Google Fonts (OFL) · github.com/google/fonts",
        fetch: Fetch::Files(&[
            (
                "https://raw.githubusercontent.com/google/fonts/main/ofl/montserrat/Montserrat%5Bwght%5D.ttf",
                "Montserrat.ttf",
            ),
            (
                "https://raw.githubusercontent.com/google/fonts/main/ofl/montserrat/Montserrat-Italic%5Bwght%5D.ttf",
                "Montserrat-Italic.ttf",
            ),
        ]),
    },
    Curated {
        family: "Roboto",
        source: "Google Fonts (OFL) · github.com/google/fonts",
        fetch: Fetch::Files(&[
            (
                "https://raw.githubusercontent.com/google/fonts/main/ofl/roboto/Roboto%5Bwdth,wght%5D.ttf",
                "Roboto.ttf",
            ),
            (
                "https://raw.githubusercontent.com/google/fonts/main/ofl/roboto/Roboto-Italic%5Bwdth,wght%5D.ttf",
                "Roboto-Italic.ttf",
            ),
        ]),
    },
    Curated {
        family: "Open Sans",
        source: "Google Fonts (OFL) · github.com/google/fonts",
        fetch: Fetch::Files(&[
            (
                "https://raw.githubusercontent.com/google/fonts/main/ofl/opensans/OpenSans%5Bwdth,wght%5D.ttf",
                "OpenSans.ttf",
            ),
            (
                "https://raw.githubusercontent.com/google/fonts/main/ofl/opensans/OpenSans-Italic%5Bwdth,wght%5D.ttf",
                "OpenSans-Italic.ttf",
            ),
        ]),
    },
    Curated {
        family: "Lato",
        source: "Google Fonts (OFL) · github.com/google/fonts",
        fetch: Fetch::Files(&[
            ("https://raw.githubusercontent.com/google/fonts/main/ofl/lato/Lato-Regular.ttf", "Lato-Regular.ttf"),
            ("https://raw.githubusercontent.com/google/fonts/main/ofl/lato/Lato-Bold.ttf", "Lato-Bold.ttf"),
            ("https://raw.githubusercontent.com/google/fonts/main/ofl/lato/Lato-Italic.ttf", "Lato-Italic.ttf"),
            ("https://raw.githubusercontent.com/google/fonts/main/ofl/lato/Lato-BoldItalic.ttf", "Lato-BoldItalic.ttf"),
        ]),
    },
    Curated {
        family: "Aptos",
        source: "Microsoft (free download) · microsoft.com/download id=106087",
        fetch: Fetch::Zip(
            "https://download.microsoft.com/download/8/6/0/860a94fa-7feb-44ef-ac79-c072d9113d69/Microsoft%20Aptos%20Fonts.zip",
        ),
    },
];

/// The curated entry for `family` (case-insensitive), if any.
fn curated(family: &str) -> Option<&'static Curated> {
    CATALOG.iter().find(|c| c.family.eq_ignore_ascii_case(family))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// One row of the Fonts settings panel. Mirrored field-for-field (snake_case) in
/// `lib/types.ts`.
#[derive(Debug, Clone, Serialize)]
pub struct FontFamily {
    /// Family name (as decks reference it).
    pub family: String,
    /// `"available"`, `"downloadable"`, or `"missing"`.
    pub status: String,
    /// Provenance of an available font, or `""` otherwise: one of `system`,
    /// `bundled`, `harvested`, `user`, `downloaded`.
    pub source: String,
    /// Whether the deck-named family is actually embedded by some deck.
    pub embedded: bool,
    /// Whether this row can be removed (app-provided: harvested/user/downloaded).
    pub removable: bool,
    /// For a downloadable family: the source shown in the consent dialog.
    pub download_source: Option<String>,
}

/// Whether any face in `db` carries `family` (case-insensitive).
fn db_has_family(db: &fontdb::Database, family: &str) -> bool {
    db.faces().any(|f| f.families.iter().any(|(fam, _)| fam.eq_ignore_ascii_case(family)))
}

/// The `<app_data>/fonts` path, for the "Reveal in Finder" affordance.
#[tauri::command]
pub async fn fonts_dir(fonts: State<'_, FontsState>) -> Result<String, String> {
    Ok(fonts.dir.to_string_lossy().into_owned())
}

/// List every font family the indexed library names, each with an availability
/// status + source, plus any app-installed family not named by a deck (so the
/// user can always remove what they added). Availability is resolved against the
/// system + bundled + app font database.
#[tauri::command]
pub async fn list_library_fonts(app: AppHandle) -> Result<Vec<FontFamily>, String> {
    let named: Vec<(String, bool)> = {
        let state = app.state::<AppState>();
        let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
        lib.library_font_families().map_err(|e| e.to_string())?
    };

    // The system+bundled+app database drives the "available on system" check.
    // Resolving it does the one-time system scan; keep it off the async runtime.
    // (Done before any FontsState borrow is taken, so nothing non-Send is held
    // across the await.)
    let db = {
        let handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || handle.state::<FontsState>().db())
            .await
            .map_err(|e| e.to_string())?
    };

    let fonts = app.state::<FontsState>();
    let mut rows: Vec<FontFamily> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (family, embedded) in named {
        seen.insert(family.to_ascii_lowercase());
        rows.push(classify(&fonts, &db, family, embedded));
    }

    // App-installed families no deck names — still listed so they're removable.
    let extra: Vec<Installed> = {
        let inner = fonts.inner.read().map_err(|_| "fonts lock poisoned")?;
        inner
            .installed
            .values()
            .filter(|i| !seen.contains(&i.family.to_ascii_lowercase()))
            .cloned()
            .collect()
    };
    for i in extra {
        rows.push(FontFamily {
            family: i.family,
            status: "available".into(),
            source: i.source.label().into(),
            embedded: false,
            removable: true,
            download_source: None,
        });
    }

    rows.sort_by_key(|r| r.family.to_ascii_lowercase());
    Ok(rows)
}

/// Classify one named family into a settings row.
fn classify(
    fonts: &FontsState,
    db: &fontdb::Database,
    family: String,
    embedded: bool,
) -> FontFamily {
    // 1. App-provided (harvested/user/downloaded) — the real font, removable.
    if let Some(inst) = fonts.installed_family(&family) {
        return FontFamily {
            family,
            status: "available".into(),
            source: inst.source.label().into(),
            embedded,
            removable: true,
            download_source: None,
        };
    }
    // 2. Installed on the system.
    if db_has_family(db, &family) {
        return FontFamily {
            family,
            status: "available".into(),
            source: "system".into(),
            embedded,
            removable: false,
            download_source: None,
        };
    }
    // 3. Covered by a bundled metric-compatible substitute (Calibri/Cambria).
    if slideflow_core::fonts::bundled_substitute(&family).is_some() {
        return FontFamily {
            family,
            status: "available".into(),
            source: "bundled".into(),
            embedded,
            removable: false,
            download_source: None,
        };
    }
    // 4. The curated resolver can fetch it.
    if let Some(c) = curated(&family) {
        return FontFamily {
            family,
            status: "downloadable".into(),
            source: String::new(),
            embedded,
            removable: false,
            download_source: Some(c.source.to_string()),
        };
    }
    // 5. Missing — the user can add it by hand.
    FontFamily {
        family,
        status: "missing".into(),
        source: String::new(),
        embedded,
        removable: false,
        download_source: None,
    }
}

/// Result of an add-fonts request: how many faces actually installed, the
/// per-file errors (KEPT even when some installed, so the frontend can surface a
/// partial failure honestly), and the refreshed family list. Mirrored
/// field-for-field (snake_case) in `lib/types.ts`.
#[derive(Debug, Clone, Serialize)]
pub struct AddFontsResult {
    pub added: u32,
    pub errors: Vec<String>,
    pub fonts: Vec<FontFamily>,
}

/// Copy validated `.ttf`/`.otf` files into `user/`, then rebuild + invalidate.
/// Returns the real installed count, any per-file errors, and the refreshed list
/// — never collapses a partial success into either a bare count or a hard error.
#[tauri::command]
pub async fn add_user_fonts(app: AppHandle, paths: Vec<String>) -> Result<AddFontsResult, String> {
    let fonts_dir = app.state::<FontsState>().dir.clone();
    let user_dir = fonts_dir.join("user");
    std::fs::create_dir_all(&user_dir).map_err(|e| e.to_string())?;

    let mut added = 0u32;
    let mut errors: Vec<String> = Vec::new();
    let mut added_families: Vec<String> = Vec::new();
    for p in paths {
        let src = PathBuf::from(&p);
        if !is_font_file(&src) {
            errors.push(format!("{p}: not a .ttf/.otf file"));
            continue;
        }
        match std::fs::read(&src) {
            Ok(bytes) if is_sfnt(&bytes) => {
                // Parse BEFORE writing: the sfnt magic check above is only 4 bytes,
                // so a truncated/corrupt face can pass it yet fail fontdb parsing.
                // Installing such a file would count it as added and wipe the
                // preview cache, but scan_dir skips it (face_identity == None), so
                // it never enters the installed map — an orphan the Settings panel
                // never lists and remove_app_font can never delete. Reject it here.
                let Some((fam, _, _)) = face_identity(&bytes) else {
                    errors.push(format!("{p}: not a parseable font"));
                    continue;
                };
                let name = src
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "font.ttf".into());
                let dest = unique_dest(&user_dir, &name);
                if let Err(e) = std::fs::write(&dest, &bytes) {
                    errors.push(format!("{p}: {e}"));
                } else {
                    added += 1;
                    added_families.push(fam);
                }
            }
            Ok(_) => errors.push(format!("{p}: not a valid TrueType/OpenType font")),
            Err(e) => errors.push(format!("{p}: {e}")),
        }
    }

    if added > 0 {
        // Explicitly (re)acquiring a family clears any tombstone from an earlier
        // removal, so a later scan may harvest it again.
        for fam in &added_families {
            clear_harvest_tombstone(&fonts_dir, fam);
        }
        app.state::<FontsState>().rebuild();
        invalidate_and_notify(&app);
    }
    let fonts = list_library_fonts(app).await?;
    Ok(AddFontsResult { added, errors, fonts })
}

/// Remove an app-installed family (all its files across harvested/user/
/// downloaded), then rebuild + invalidate. Errors if the family isn't
/// app-provided (a system/bundled font can't be removed here).
#[tauri::command]
pub async fn remove_app_font(app: AppHandle, family: String) -> Result<Vec<FontFamily>, String> {
    let fonts_dir = app.state::<FontsState>().dir.clone();
    let Some(inst) = app.state::<FontsState>().installed_family(&family) else {
        return Err(format!("{family} is not an app-added font"));
    };
    let harvested_dir = fonts_dir.join("harvested");
    let mut was_harvested = false;
    for f in &inst.files {
        if f.starts_with(&harvested_dir) {
            was_harvested = true;
        }
        let _ = std::fs::remove_file(f);
    }
    // Tombstone a removed harvested family so the next scan doesn't re-harvest it
    // from the same deck (and re-wipe the preview cache). User/downloaded families
    // aren't auto-re-added, so they need no tombstone.
    if was_harvested {
        tombstone_harvested_family(&fonts_dir, &inst.family);
    }
    app.state::<FontsState>().rebuild();
    invalidate_and_notify(&app);
    list_library_fonts(app).await
}

/// Download-lifecycle events on `font:download`. `canceled` is distinct from
/// `error` so the UI resets quietly after a user cancel. Mirrored in
/// `lib/types.ts`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FontDownloadEvent {
    Started { family: String },
    Done { family: String },
    Canceled { family: String },
    Error { family: String, message: String },
}

fn emit_download(app: &AppHandle, ev: &FontDownloadEvent) {
    let _ = app.emit("font:download", ev);
}

/// Fetch a curated family into `downloaded/` on a background thread (consent is
/// obtained by the frontend before this is called). Returns `Ok(false)` when a
/// download is already running, or the family isn't in the catalog handled as an
/// error. Progress/result arrive on `font:download`.
#[tauri::command]
pub async fn download_font(app: AppHandle, family: String) -> Result<bool, String> {
    let fonts = app.state::<FontsState>();
    let Some(entry) = curated(&family) else {
        return Err(format!("No known download source for {family}"));
    };
    if fonts
        .downloading
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(false); // already downloading — not an error
    }
    fonts.download_cancel.store(false, Ordering::SeqCst);
    fonts.set_error(None);

    let app_for_thread = app.clone();
    let family = entry.family.to_string();
    std::thread::spawn(move || {
        let fonts = app_for_thread.state::<FontsState>();
        emit_download(&app_for_thread, &FontDownloadEvent::Started { family: family.clone() });
        let result = run_font_download(&fonts, entry);
        fonts.downloading.store(false, Ordering::SeqCst);
        match result {
            Ok(true) => {
                // Explicitly (re)acquiring a family clears any tombstone from an
                // earlier removal, so a later scan may harvest it again.
                clear_harvest_tombstone(&fonts.dir, &family);
                fonts.rebuild();
                invalidate_and_notify(&app_for_thread);
                emit_download(&app_for_thread, &FontDownloadEvent::Done { family });
            }
            Ok(false) => {
                emit_download(&app_for_thread, &FontDownloadEvent::Canceled { family });
            }
            Err(message) => {
                fonts.set_error(Some(message.clone()));
                emit_download(&app_for_thread, &FontDownloadEvent::Error { family, message });
            }
        }
    });
    Ok(true)
}

#[tauri::command]
pub async fn cancel_font_download(fonts: State<'_, FontsState>) -> Result<(), String> {
    fonts.download_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// Do the actual fetch for one curated family. Returns `Ok(false)` on cancel.
fn run_font_download(fonts: &FontsState, entry: &Curated) -> Result<bool, String> {
    let dest_dir = fonts.dir.join("downloaded");
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    // Fonts are small — a whole-request timeout is fine (unlike the ~490 MB
    // model). The connect timeout still catches unreachable hosts.
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let cancelled = || fonts.download_cancel.load(Ordering::SeqCst);

    match &entry.fetch {
        Fetch::Files(files) => {
            let mut wrote = 0usize;
            for (url, name) in *files {
                if cancelled() {
                    return Ok(false);
                }
                let bytes = get_bytes(&client, url)?;
                if !is_sfnt(&bytes) {
                    return Err(format!("{name}: downloaded data is not a font"));
                }
                let dest = dest_dir.join(name);
                write_atomic(&dest, &bytes).map_err(|e| format!("{name}: {e}"))?;
                wrote += 1;
            }
            if wrote == 0 {
                return Err("nothing to download".into());
            }
        }
        Fetch::Zip(url) => {
            if cancelled() {
                return Ok(false);
            }
            let zip_bytes = get_bytes(&client, url)?;
            let wrote = unpack_font_zip(&zip_bytes, &dest_dir, entry.family)?;
            if wrote == 0 {
                return Err("the download contained no usable fonts".into());
            }
        }
    }
    Ok(true)
}

/// GET `url` fully into memory, erroring on any non-success status.
fn get_bytes(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>, String> {
    let resp = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("{url}: {e}"))?;
    let bytes = resp.bytes().map_err(|e| format!("{url}: {e}"))?;
    Ok(bytes.to_vec())
}

/// Unpack every `.ttf`/`.otf` entry of a font zip into `dest_dir`, keeping only
/// those whose bytes validate as a font AND belong to `family` (so Microsoft's
/// Aptos zip yields the Aptos faces, not any bundled extras). Returns the count
/// written.
fn unpack_font_zip(zip_bytes: &[u8], dest_dir: &Path, family: &str) -> Result<usize, String> {
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("zip: {e}"))?;
    let mut wrote = 0usize;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| format!("zip: {e}"))?;
        if !file.is_file() {
            continue;
        }
        let name = file.name().to_string();
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".ttf") || lower.ends_with(".otf")) {
            continue;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| format!("{name}: {e}"))?;
        if !is_sfnt(&bytes) {
            continue;
        }
        // Keep only faces of the requested family (the zip may carry siblings).
        let belongs = face_identity(&bytes)
            .map(|(fam, _, _)| fam.eq_ignore_ascii_case(family) || fam.to_ascii_lowercase().contains(&family.to_ascii_lowercase()))
            .unwrap_or(false);
        if !belongs {
            continue;
        }
        let base = Path::new(&name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{family}-{i}.ttf"));
        let dest = unique_dest(dest_dir, &base);
        write_atomic(&dest, &bytes).map_err(|e| format!("{base}: {e}"))?;
        wrote += 1;
    }
    Ok(wrote)
}

/// A destination path in `dir` for `name`, suffixed ` (2)`, ` (3)`, … on
/// collision so re-adding a same-named file never clobbers an existing one.
fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("font");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("ttf");
    let mut n = 2;
    loop {
        let candidate = dir.join(format!("{stem} ({n}).{ext}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Write `bytes` to `path` via a temp sibling + rename, so a crash or a
/// concurrent scan never sees a half-written font.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("part");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        // Flush to stable storage BEFORE the rename so a crash can't leave the
        // renamed destination as an empty/truncated file under a valid .ttf name
        // (which the next scan_dir would silently reject, dropping the face). The
        // previous seek(Start(0)) here flushed nothing.
        f.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Harvest tombstone (families the user removed, so a rescan doesn't re-add them)
// ---------------------------------------------------------------------------

/// The tombstone file: `<fonts_dir>/harvested/.removed`, a JSON array of
/// lowercased family names the user removed. Without it, deleting a harvested
/// face doesn't stick — the next scan re-harvests it from the same deck (the
/// content-addressed file is gone, so `dest.exists()` is false) and wipes the
/// whole preview cache every time. It has no font extension, so `scan_dir` never
/// mistakes it for a face.
fn harvest_tombstone_path(dir: &Path) -> PathBuf {
    dir.join("harvested").join(".removed")
}

fn read_harvest_tombstone(dir: &Path) -> std::collections::HashSet<String> {
    std::fs::read_to_string(harvest_tombstone_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(|v| v.into_iter().map(|f| f.to_ascii_lowercase()).collect())
        .unwrap_or_default()
}

fn write_harvest_tombstone(dir: &Path, families: &std::collections::HashSet<String>) {
    let path = harvest_tombstone_path(dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut list: Vec<&String> = families.iter().collect();
    list.sort(); // deterministic on disk
    if let Ok(json) = serde_json::to_string(&list) {
        let _ = std::fs::write(&path, json);
    }
}

/// Tombstone `family` (called when a harvested face is removed) so the next scan
/// skips re-harvesting it.
fn tombstone_harvested_family(dir: &Path, family: &str) {
    let mut set = read_harvest_tombstone(dir);
    if set.insert(family.to_ascii_lowercase()) {
        write_harvest_tombstone(dir, &set);
    }
}

/// Drop `family` from the tombstone (called when it's explicitly added or
/// downloaded), so it becomes harvestable again.
fn clear_harvest_tombstone(dir: &Path, family: &str) {
    let mut set = read_harvest_tombstone(dir);
    if set.remove(&family.to_ascii_lowercase()) {
        write_harvest_tombstone(dir, &set);
    }
}

// ---------------------------------------------------------------------------
// Harvest embedded fonts after a scan
// ---------------------------------------------------------------------------

/// After a scan, copy every harvestable EMBEDDED font of the indexed decks into
/// `harvested/`, so any deck naming that family renders/exports with the real
/// face. Only decks that embed a font are reopened (from the `deck_fonts`
/// inventory), so this is cheap on a library that embeds nothing. If a genuinely
/// new face lands, the font set is rebuilt and the preview cache invalidated.
pub fn spawn_harvest_after_scan(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let paths = {
            let state = app.state::<AppState>();
            let Ok(lib) = state.library.lock() else { return };
            lib.decks_with_embedded_fonts().unwrap_or_default()
        };
        if paths.is_empty() {
            return;
        }
        let fonts = app.state::<FontsState>();
        let harvested_dir = fonts.dir.join("harvested");
        let tombstoned = read_harvest_tombstone(&fonts.dir);
        let mut changed = false;
        for path in paths {
            let Ok(pf) = PresentationFile::open(Path::new(&path)) else { continue };
            for f in &pf.embedded_font_set().fonts {
                if tombstoned.contains(&f.family.to_ascii_lowercase()) {
                    // The user removed this harvested family — don't silently
                    // re-add it (leaving `changed` false so no cache wipe). Adding
                    // or downloading the family clears the tombstone.
                    continue;
                }
                if !is_harvestable(&f.bytes) {
                    // Preview/print-only or restricted embedding — must not be
                    // reused app-wide. Left in its own deck only.
                    #[cfg(debug_assertions)]
                    eprintln!("[fonts] skip harvest of {} (fsType forbids reuse)", f.family);
                    continue;
                }
                let name = harvested_name(&f.family, f.bold, f.italic, &f.bytes);
                let dest = harvested_dir.join(&name);
                if dest.exists() {
                    continue; // content-addressed — already harvested
                }
                if std::fs::create_dir_all(&harvested_dir).is_ok()
                    && write_atomic(&dest, &f.bytes).is_ok()
                {
                    changed = true;
                }
            }
        }
        if changed {
            fonts.rebuild();
            invalidate_and_notify(&app);
        }
    });
}

/// A content-addressed file name for a harvested face: `<family>-<variant>-
/// <hash8>.ttf`. The hash dedupes identical faces across decks and keeps the
/// `dest.exists()` skip stable across rescans; the family/variant keep it
/// human-legible and let a remove match by family.
fn harvested_name(family: &str, bold: bool, italic: bool, bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let variant = match (bold, italic) {
        (false, false) => "Regular",
        (true, false) => "Bold",
        (false, true) => "Italic",
        (true, true) => "BoldItalic",
    };
    let hash: String = Sha256::digest(bytes).iter().take(4).map(|b| format!("{b:02x}")).collect();
    let safe: String = family
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    format!("{safe}-{variant}-{hash}.ttf")
}

// ---------------------------------------------------------------------------
// Cache invalidation
// ---------------------------------------------------------------------------

/// On any font-set change: wipe the preview cache (its keys don't know the app
/// font set, so every render is now stale) and tell the frontend to drop its
/// session SVG cache and re-render via `fonts:changed`.
pub fn invalidate_and_notify(app: &AppHandle) {
    let state = app.state::<AppState>();
    let _ = std::fs::remove_dir_all(&state.thumbs_dir);
    let _ = std::fs::create_dir_all(&state.thumbs_dir);
    // Drag-out icons are keyed by deck mtime (not the font set), so a font change
    // leaves them stale too — wipe + recreate the dir the same way (mirrors the
    // startup wipe in lib.rs) so the next drag re-renders with the new fonts.
    let _ = std::fs::remove_dir_all(&state.dragout_dir);
    let _ = std::fs::create_dir_all(&state.dragout_dir);
    let _ = app.emit("fonts:changed", ());
}

#[cfg(test)]
mod tests {
    use super::{face_identity, is_sfnt, write_atomic};

    /// A unique scratch path under the OS temp dir (no tempfile dep in this crate).
    fn scratch(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("slideflow-fonts-test-{pid}-{tag}-{n}"))
    }

    #[test]
    fn write_atomic_persists_bytes_and_leaves_no_part() {
        // Guards the seek→sync_all fix: the durable write must still land the exact
        // bytes at the destination and clean up its temp sibling.
        let dest = scratch("atomic").with_extension("ttf");
        let part = dest.with_extension("part");
        let payload = b"\x00\x01\x00\x00 not a real font but exact bytes";
        write_atomic(&dest, payload).expect("atomic write");
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        assert!(!part.exists(), "temp .part must not survive a successful write");
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    fn sfnt_magic_but_unparseable_is_rejected_by_face_identity() {
        // The exact class add_user_fonts now rejects BEFORE writing: a file that
        // passes the 4-byte sfnt magic check but that fontdb cannot parse. If this
        // ever returned Some, an unparseable orphan could be installed again.
        let mut bytes = vec![0x00, 0x01, 0x00, 0x00];
        bytes.extend_from_slice(b"truncated garbage, no valid tables");
        assert!(is_sfnt(&bytes), "precondition: passes the sfnt magic gate");
        assert!(
            face_identity(&bytes).is_none(),
            "unparseable font must yield None so add_user_fonts rejects it"
        );
    }
}
