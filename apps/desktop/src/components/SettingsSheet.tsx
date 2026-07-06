import { useEffect, useState, type ReactNode } from "react";
import {
  X,
  Monitor,
  Sun,
  Moon,
  Minus,
  Plus,
  Folder,
  FolderPlus,
  FolderOpen,
  Download,
  Trash2,
} from "lucide-react";
import { useApp, type ThemeMode } from "../stores/useApp";
import { useUpdater } from "../stores/useUpdater";
import { useSemantic } from "../stores/useSemantic";
import { useFonts } from "../stores/useFonts";
import type { FontFamily } from "../lib/types";
import { basename, cx } from "../lib/utils";
import * as api from "../lib/api";
import { toast } from "../stores/useToast";
import { UpdateStatus } from "./AboutSheet";
import RootExcludesEditor from "./RootExcludesEditor";
import OverlaySheet from "./OverlaySheet";

const AUTO_UPDATE_KEY = "slideflow.autoUpdate.v1";

/** Preferences sheet: appearance, library, and update settings. Follows the
 *  AboutSheet overlay idiom (backdrop + spring card, reduced-motion aware).
 *  Opened by ⌘, or the command palette; closes via backdrop / X only. */
export default function SettingsSheet() {
  const open = useApp((s) => s.settingsOpen);
  const close = () => useApp.getState().setSettingsOpen(false);

  return (
    <OverlaySheet
      open={open}
      onClose={close}
      cardClassName="max-w-md"
      onCardKeyDown={(e) => {
        // Escape closes the sheet unless a field has focus (there, let the
        // field keep the key — don't nuke an in-progress exclude-glob edit).
        const tag = (e.target as HTMLElement).tagName;
        const editing = tag === "INPUT" || tag === "TEXTAREA";
        if (e.key === "Escape" && !editing) close();
      }}
    >
      <div className="flex items-center justify-between px-5 py-3 hairline-b">
              <div className="text-title font-semibold text-ink">Settings</div>
              <button
                onClick={close}
                aria-label="Close"
                className="rounded-full p-1 text-subtle hover:bg-ink/10"
              >
                <X size={16} />
              </button>
            </div>

            <div className="max-h-[70vh] space-y-6 overflow-y-auto px-5 py-5">
              <AppearanceSection />
              <LibrarySection />
              <FontsSection />
              <UpdatesSection />
              <SemanticSection />
            </div>
    </OverlaySheet>
  );
}

/** Uppercase section header, matching the Sidebar section-label styling. */
function SectionLabel({ children }: { children: ReactNode }) {
  return (
    <div className="px-0 pb-2 text-caption font-semibold uppercase tracking-wide text-subtle/70">
      {children}
    </div>
  );
}

const THEME_OPTIONS: Array<[ThemeMode, typeof Monitor, string]> = [
  ["system", Monitor, "System"],
  ["light", Sun, "Light"],
  ["dark", Moon, "Dark"],
];

function AppearanceSection() {
  const theme = useApp((s) => s.theme);
  const gridCols = useApp((s) => s.gridCols);

  return (
    <div>
      <SectionLabel>Appearance</SectionLabel>

      <div className="mb-4">
        <div className="mb-1.5 text-body text-ink">Theme</div>
        <div className="flex gap-0.5 rounded-[8px] bg-ink/5 p-0.5">
          {THEME_OPTIONS.map(([mode, Icon, label]) => (
            <button
              key={mode}
              onClick={() => useApp.getState().setTheme(mode)}
              className={cx(
                "flex flex-1 items-center justify-center gap-1.5 rounded-[6px] px-2 py-1.5 text-body",
                theme === mode ? "bg-accent text-white" : "text-subtle hover:text-ink",
              )}
            >
              <Icon size={14} /> {label}
            </button>
          ))}
        </div>
      </div>

      <div className="flex items-center justify-between">
        <div className="text-body text-ink">Grid columns</div>
        <div className="flex items-center gap-1">
          <button
            onClick={() => useApp.getState().decCols()}
            disabled={gridCols <= 3}
            aria-label="Fewer columns"
            className="rounded-[6px] p-1 text-subtle hover:bg-ink/10 disabled:opacity-40 disabled:hover:bg-transparent"
          >
            <Minus size={14} />
          </button>
          <span className="tabnum w-6 text-center text-body">{gridCols}</span>
          <button
            onClick={() => useApp.getState().incCols()}
            disabled={gridCols >= 10}
            aria-label="More columns"
            className="rounded-[6px] p-1 text-subtle hover:bg-ink/10 disabled:opacity-40 disabled:hover:bg-transparent"
          >
            <Plus size={14} />
          </button>
        </div>
      </div>
      <p className="mt-1 text-caption text-subtle">
        Fewer columns show larger thumbnails.
      </p>
    </div>
  );
}

function LibrarySection() {
  const roots = useApp((s) => s.roots);

  return (
    <div>
      <SectionLabel>Library</SectionLabel>

      <div className="space-y-0.5">
        {roots.map((r) => (
          <div key={r.id}>
            <div className="flex items-center gap-2 rounded-[6px] px-2 py-1.5 hover:bg-ink/5">
              <Folder size={15} className="shrink-0 text-subtle" />
              <div className="min-w-0 flex-1">
                <div className="truncate text-body text-ink">{basename(r.path)}</div>
                <div className="truncate text-caption text-subtle" title={r.path}>
                  {r.path}
                </div>
              </div>
              <span className="tabnum text-caption text-subtle">{r.slide_count}</span>
              <button
                aria-label="Remove folder"
                onClick={() => void useApp.getState().removeRoot(r.id)}
                className="rounded-[6px] p-1 text-subtle hover:text-ink"
              >
                <Trash2 size={14} />
              </button>
            </div>
            {/* Per-root exclude-glob editor (step4 #17): reads r.exclude_globs,
                calls setRootExcludes then re-scans. */}
            <RootExcludesEditor root={r} />
          </div>
        ))}
      </div>

      <button
        onClick={() => void useApp.getState().addFolder()}
        className="mt-2 flex items-center gap-2 rounded-[6px] px-2 py-1.5 text-body text-subtle hover:text-ink"
      >
        <FolderPlus size={15} /> Add folder…
      </button>

      <button
        onClick={() => useApp.getState().confirmClearAndRebuild()}
        className="mt-3 w-full rounded-[8px] border border-hairline/15 px-4 py-2 text-body font-medium text-red-500 hover:bg-red-500/10"
      >
        Clear &amp; rebuild index…
      </button>
    </div>
  );
}

function UpdatesSection() {
  const phase = useUpdater((s) => s.phase);
  const [auto, setAuto] = useState<boolean>(
    () => localStorage.getItem(AUTO_UPDATE_KEY) !== "0",
  );

  // Reconcile the toggle against the backend file the scheduler actually gates
  // on (localStorage is only an optimistic cache and can drift, e.g. webview
  // storage cleared while the config file persists). Runs each time the sheet
  // opens, since this section mounts with it.
  useEffect(() => {
    let alive = true;
    api
      .getAutoUpdateEnabled()
      .then((enabled) => {
        if (!alive) return;
        setAuto(enabled);
        localStorage.setItem(AUTO_UPDATE_KEY, enabled ? "1" : "0");
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  const onToggle = () => {
    const next = !auto;
    setAuto(next);
    api
      .setAutoUpdateEnabled(next)
      .then(() => localStorage.setItem(AUTO_UPDATE_KEY, next ? "1" : "0"))
      .catch((err) => {
        setAuto(auto);
        toast.error("Couldn't change automatic updates: " + String(err));
      });
  };

  return (
    <div>
      <SectionLabel>Updates</SectionLabel>

      {phase !== "unsupported" && (
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="text-body text-ink">Automatic updates</div>
            <p className="text-caption text-subtle">
              Check for updates on launch and daily.
            </p>
          </div>
          <Switch checked={auto} onToggle={onToggle} />
        </div>
      )}

      <UpdateStatus />
    </div>
  );
}

/** The shared switch control (extracted from the auto-update toggle). */
function Switch({ checked, onToggle }: { checked: boolean; onToggle: () => void }) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      onClick={onToggle}
      className={cx(
        "relative h-[22px] w-[38px] shrink-0 rounded-full transition-colors",
        checked ? "bg-accent" : "bg-ink/20",
      )}
    >
      {/* left-[2px] anchors the knob; without it the browser's static-position
          fallback puts the base X mid-track and the translate pushes the knob
          outside (invisible white-on-white when checked). */}
      <span
        className={cx(
          "absolute left-[2px] top-[2px] h-[18px] w-[18px] rounded-full bg-white transition-transform",
          checked ? "translate-x-[16px]" : "translate-x-0",
        )}
      />
    </button>
  );
}

/** Fonts: the families the indexed library names, each with an availability
 *  status. Missing families can be added (a licensed .ttf/.otf) or downloaded
 *  (curated, with per-download consent); app-provided fonts can be removed. All
 *  fonts live under <app_data>/fonts and are never installed system-wide. */
function FontsSection() {
  const fonts = useFonts((s) => s.fonts);
  const loaded = useFonts((s) => s.loaded);
  const downloading = useFonts((s) => s.downloading);
  const dir = useFonts((s) => s.dir);

  // Refresh when the sheet opens so the list reflects the latest scan/harvest.
  useEffect(() => {
    void useFonts.getState().refresh();
  }, []);

  const confirmDownload = (family: string, source: string | null) =>
    useApp.getState().requestConfirm({
      title: `Download ${family}?`,
      message: `Slideflow will download ${family} from its official source${
        source ? `:\n\n${source}` : ""
      }.\n\nIt's stored only for this app, never installed system-wide.`,
      confirmLabel: "Download",
      onConfirm: () => void useFonts.getState().download(family),
    });

  const confirmRemove = (family: string) =>
    useApp.getState().requestConfirm({
      title: `Remove ${family}?`,
      message: `This deletes Slideflow's copy of ${family}. Decks that use it fall back to a substitute until you re-add or re-download it.`,
      confirmLabel: "Remove",
      destructive: true,
      onConfirm: () => void useFonts.getState().remove(family),
    });

  return (
    <div>
      <SectionLabel>Fonts</SectionLabel>

      {loaded && fonts.length === 0 ? (
        <p className="text-caption text-subtle">
          No fonts detected yet. Add a folder of decks and scan to build the list.
        </p>
      ) : (
        <div className="space-y-0.5">
          {fonts.map((f) => (
            <FontRow
              key={f.family}
              font={f}
              downloading={downloading === f.family}
              onDownload={() => confirmDownload(f.family, f.download_source)}
              onRemove={() => confirmRemove(f.family)}
            />
          ))}
        </div>
      )}

      <button
        onClick={() => void useFonts.getState().addFonts()}
        className="mt-2 flex items-center gap-2 rounded-[6px] px-2 py-1.5 text-body text-subtle hover:text-ink"
      >
        <FolderPlus size={15} /> Add font…
      </button>

      {dir && (
        <div className="mt-2 flex items-center gap-2 px-2">
          <div className="min-w-0 flex-1 truncate text-caption text-subtle" title={dir}>
            {dir}
          </div>
          <button
            onClick={() => void useFonts.getState().revealFolder()}
            className="flex shrink-0 items-center gap-1 rounded-[6px] px-1.5 py-0.5 text-caption text-subtle hover:bg-ink/8 hover:text-ink"
          >
            <FolderOpen size={13} /> Reveal
          </button>
        </div>
      )}
    </div>
  );
}

/** One font row: a status dot + family + source, and the applicable action
 *  (Download / Add… / Remove). */
function FontRow({
  font,
  downloading,
  onDownload,
  onRemove,
}: {
  font: FontFamily;
  downloading: boolean;
  onDownload: () => void;
  onRemove: () => void;
}) {
  const statusLabel =
    font.status === "available"
      ? `Available${font.source ? ` · ${font.source}` : ""}`
      : font.status === "downloadable"
        ? "Downloadable"
        : "Not installed";
  const dotClass =
    font.status === "available"
      ? "bg-green-500"
      : font.status === "downloadable"
        ? "bg-accent"
        : "bg-ink/30";

  return (
    <div className="flex items-center gap-2 rounded-[6px] px-2 py-1.5 hover:bg-ink/5">
      <span className={cx("h-1.5 w-1.5 shrink-0 rounded-full", dotClass)} />
      <div className="min-w-0 flex-1">
        <div className="truncate text-body text-ink">
          {font.family}
          {font.embedded && (
            <span className="ml-1.5 align-middle text-caption text-subtle">embedded</span>
          )}
        </div>
        <div className="truncate text-caption text-subtle">{statusLabel}</div>
      </div>

      {font.status === "downloadable" &&
        (downloading ? (
          <button
            onClick={() => void useFonts.getState().cancelDownload()}
            className="rounded-[6px] px-1.5 py-0.5 text-caption text-subtle hover:bg-ink/8 hover:text-ink"
          >
            Cancel
          </button>
        ) : (
          <button
            onClick={onDownload}
            aria-label={`Download ${font.family}`}
            className="flex shrink-0 items-center gap-1 rounded-[6px] px-2 py-1 text-caption font-medium text-accent hover:bg-accent/10"
          >
            <Download size={13} /> Download
          </button>
        ))}

      {font.status === "missing" && (
        <button
          onClick={() => void useFonts.getState().addFonts()}
          className="shrink-0 rounded-[6px] px-2 py-1 text-caption text-subtle hover:text-ink"
        >
          Add…
        </button>
      )}

      {font.removable && (
        <button
          onClick={onRemove}
          aria-label={`Remove ${font.family}`}
          className="shrink-0 rounded-[6px] p-1 text-subtle hover:text-ink"
        >
          <Trash2 size={14} />
        </button>
      )}
    </div>
  );
}

/** AI: the semantic-search toggle, model download (with explicit consent — a
 *  ~490 MB one-time download), indexing progress, and model management. */
function SemanticSection() {
  const status = useSemantic((s) => s.status);
  const downloadProgress = useSemantic((s) => s.downloadProgress);
  const indexing = useSemantic((s) => s.indexing);

  const state = status?.state ?? "disabled";
  const enabled = state !== "disabled";

  const confirmDownload = () =>
    useApp.getState().requestConfirm({
      title: "Download semantic search model?",
      message:
        "Slideflow will download the multilingual-e5-small model (a one-time download of about 490 MB) from huggingface.co. After that, semantic search and indexing run entirely on this Mac — your slides never leave it.",
      confirmLabel: "Download model",
      onConfirm: () => void useSemantic.getState().download(),
      // Consent semantics: "no" means OFF. Declining (cancel/backdrop/Escape)
      // reverts the just-flipped toggle instead of leaving the feature enabled
      // but undownloaded.
      onCancel: () => void useSemantic.getState().setEnabled(false),
    });

  const onToggle = async () => {
    const sem = useSemantic.getState();
    if (enabled) {
      await sem.setEnabled(false);
      return;
    }
    await sem.setEnabled(true);
    // Enabling without the model on disk → ask before pulling ~490 MB. The
    // consent dialog's onCancel reverts the toggle, so a declined consent
    // leaves the feature exactly as it was: off.
    if (useSemantic.getState().status?.state === "not_downloaded") {
      confirmDownload();
    }
  };

  const confirmDelete = () =>
    useApp.getState().requestConfirm({
      title: "Delete semantic search model?",
      message:
        "This removes the downloaded model files (about 490 MB) from disk and turns semantic search off. Your slides and search index are unaffected.",
      confirmLabel: "Delete model",
      destructive: true,
      onConfirm: () => void useSemantic.getState().deleteModel(),
    });

  const pct = downloadProgress != null ? Math.round(downloadProgress * 100) : null;

  return (
    <div>
      <SectionLabel>AI</SectionLabel>

      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="text-body text-ink">Semantic search</div>
          <p className="text-caption text-subtle">
            Find slides by meaning, across languages. Runs fully on this Mac.
          </p>
        </div>
        <Switch checked={enabled} onToggle={() => void onToggle()} />
      </div>

      {state === "downloading" && (
        <div className="mt-3">
          <div className="flex items-center justify-between text-caption text-subtle">
            <span>Downloading model{pct != null ? ` — ${pct}%` : "…"}</span>
            <button
              onClick={() => void useSemantic.getState().cancelDownload()}
              className="rounded-[6px] px-1.5 py-0.5 text-caption text-subtle hover:bg-ink/8 hover:text-ink"
            >
              Cancel
            </button>
          </div>
          <div className="mt-1.5 h-1 overflow-hidden rounded-full bg-ink/10">
            <div
              className="h-full rounded-full bg-accent transition-[width] duration-300"
              style={{ width: pct != null ? `${pct}%` : "35%" }}
            />
          </div>
        </div>
      )}

      {state === "not_downloaded" && (
        <button
          onClick={confirmDownload}
          className="mt-3 w-full rounded-[8px] border border-hairline/15 px-4 py-2 text-body font-medium text-accent hover:bg-accent/10"
        >
          Download model (≈490 MB)…
        </button>
      )}

      {state === "error" && status?.error && (
        <div className="mt-3">
          <p className="text-caption text-red-500">{status.error}</p>
          <button
            onClick={() => void useSemantic.getState().download()}
            className="mt-2 w-full rounded-[8px] border border-hairline/15 px-4 py-2 text-body font-medium text-accent hover:bg-accent/10"
          >
            Retry download
          </button>
        </div>
      )}

      {state === "ready" && status && (
        <div className="mt-3">
          <p className="tabnum text-caption text-subtle">
            Model: {status.model_id} · {status.embedded_slides} of {status.total_slides} slides
            indexed
          </p>
          {indexing && (
            <div className="mt-1.5">
              <div className="tabnum text-caption text-subtle">
                Indexing slides… {indexing.done} of {indexing.total}
              </div>
              <div className="mt-1 h-1 overflow-hidden rounded-full bg-ink/10">
                <div
                  className="h-full rounded-full bg-accent transition-[width] duration-300"
                  style={{
                    width: indexing.total
                      ? `${Math.min(100, (indexing.done / indexing.total) * 100)}%`
                      : "35%",
                  }}
                />
              </div>
            </div>
          )}
          <div className="mt-2 flex gap-2">
            <button
              onClick={() => void useSemantic.getState().reindex()}
              disabled={indexing != null}
              className="flex-1 rounded-[8px] border border-hairline/15 px-3 py-1.5 text-body text-ink hover:bg-ink/5 disabled:opacity-40"
            >
              Re-run indexing
            </button>
            <button
              onClick={confirmDelete}
              className="flex-1 rounded-[8px] border border-hairline/15 px-3 py-1.5 text-body text-red-500 hover:bg-red-500/10"
            >
              Delete model…
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
