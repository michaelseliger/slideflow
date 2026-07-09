import { useEffect, useState, type ReactNode } from "react";
import {
  X,
  SunMoon,
  Folder,
  FolderPlus,
  FolderOpen,
  Download,
  Trash2,
  Type,
  Sparkles,
  RefreshCw,
  Loader2,
  SlidersHorizontal,
  Terminal,
} from "lucide-react";
import { useApp, type ThemeMode } from "../stores/useApp";
import { useUpdater } from "../stores/useUpdater";
import { useSemantic } from "../stores/useSemantic";
import { useFonts } from "../stores/useFonts";
import type { FontFamily } from "../lib/types";
import { cx } from "../lib/utils";
import * as api from "../lib/api";
import { toast } from "../stores/useToast";
import RootExcludesEditor from "./RootExcludesEditor";
import OverlaySheet from "./OverlaySheet";

const AUTO_UPDATE_KEY = "slideflow.autoUpdate.v1";
const LAST_CHECK_KEY = "slideflow.updateLastChecked.v1";

type SectionKey = "appearance" | "library" | "fonts" | "updates" | "ai" | "advanced";

const NAV: { key: SectionKey; label: string; icon: ReactNode }[] = [
  { key: "appearance", label: "Appearance", icon: <SunMoon size={15} /> },
  { key: "library", label: "Library", icon: <Folder size={15} /> },
  { key: "fonts", label: "Fonts", icon: <Type size={15} /> },
  { key: "updates", label: "Updates", icon: <Download size={15} /> },
  { key: "ai", label: "Semantic search", icon: <Sparkles size={15} /> },
  { key: "advanced", label: "Advanced", icon: <Terminal size={15} /> },
];

/** Preferences sheet: a two-pane master/detail (nav + section). Opened by ⌘, or
 *  the command palette; closes via backdrop / X / Escape. */
export default function SettingsSheet() {
  const open = useApp((s) => s.settingsOpen);
  const close = () => useApp.getState().setSettingsOpen(false);
  const [section, setSection] = useState<SectionKey>("appearance");

  return (
    <OverlaySheet
      open={open}
      onClose={close}
      cardClassName="max-w-[640px]"
      onCardKeyDown={(e) => {
        // Escape closes the sheet unless a field has focus (there, let the field
        // keep the key — don't nuke an in-progress exclude-glob edit).
        const tag = (e.target as HTMLElement).tagName;
        const editing = tag === "INPUT" || tag === "TEXTAREA";
        if (e.key === "Escape" && !editing) close();
      }}
    >
      <div className="flex items-center px-4 py-3.5 hairline-b">
        <span className="text-title font-semibold text-ink">Settings</span>
        <button
          onClick={close}
          aria-label="Close"
          className="ml-auto flex text-subtle hover:text-ink"
        >
          <X size={16} />
        </button>
      </div>

      <div className="flex h-[520px]">
        <nav className="flex w-[180px] shrink-0 flex-col gap-0.5 hairline-r p-2.5">
          {NAV.map((n) => {
            const active = n.key === section;
            return (
              <button
                key={n.key}
                onClick={() => setSection(n.key)}
                className={cx(
                  "flex items-center gap-2.5 rounded-[6px] px-2.5 py-[7px] text-body",
                  active ? "bg-accent/[0.14] text-accent" : "text-ink hover:bg-ink/5",
                )}
              >
                <span className={active ? "text-accent" : "text-subtle"}>{n.icon}</span>
                {n.label}
              </button>
            );
          })}
        </nav>

        <div className="min-w-0 flex-1 overflow-y-auto px-5 py-[18px]">
          {section === "appearance" && <AppearanceSection />}
          {section === "library" && <LibrarySection />}
          {section === "fonts" && <FontsSection />}
          {section === "updates" && <UpdatesSection />}
          {section === "ai" && <SemanticSection />}
          {section === "advanced" && <AdvancedSection />}
        </div>
      </div>
    </OverlaySheet>
  );
}

/** Section heading (15px, sentence case). */
function SectionHeading({ children, aside }: { children: ReactNode; aside?: ReactNode }) {
  return (
    <div className="mb-3.5 flex items-center">
      <span className="text-title font-semibold text-ink">{children}</span>
      {aside && <span className="ml-auto flex items-center gap-1.5 text-caption text-subtle">{aside}</span>}
    </div>
  );
}

/** Small uppercase label for sub-groups within a section. */
function GroupLabel({ children }: { children: ReactNode }) {
  return (
    <div className="mb-2 text-caption font-semibold uppercase tracking-wide text-subtle/70">
      {children}
    </div>
  );
}

/** A settings row: title (+ optional subtitle) on the left, control on the right. */
function Row({
  title,
  subtitle,
  control,
  divider = true,
}: {
  title: string;
  subtitle?: string;
  control: ReactNode;
  divider?: boolean;
}) {
  return (
    <div className={cx("flex items-center gap-3 py-3", divider && "hairline-b")}>
      <div className="min-w-0 flex-1">
        <div className="text-body font-medium text-ink">{title}</div>
        {subtitle && <div className="mt-0.5 text-caption text-subtle">{subtitle}</div>}
      </div>
      <div className="shrink-0">{control}</div>
    </div>
  );
}

/** iOS-style switch, matching the design toggle (38×22, accent track). */
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
      <span
        className={cx(
          "absolute left-[2px] top-[2px] h-[18px] w-[18px] rounded-full bg-white transition-transform",
          checked ? "translate-x-[16px]" : "translate-x-0",
        )}
      />
    </button>
  );
}

/** A compact bordered segmented control (accent-filled active segment). */
function Segmented<T extends string>({
  options,
  value,
  onChange,
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (v: T) => void;
}) {
  return (
    <div className="inline-flex overflow-hidden rounded-[6px] border border-hairline/[0.12]">
      {options.map((o) => (
        <button
          key={o.value}
          onClick={() => onChange(o.value)}
          className={cx(
            "px-3 py-[5px] text-caption",
            o.value === value
              ? "bg-accent font-semibold text-white"
              : "font-medium text-subtle hover:bg-ink/5",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

const dangerGhost =
  "rounded-[6px] border border-red-500/30 px-3 py-1.5 text-caption font-medium text-red-500 hover:bg-red-500/10";
const outlineBtn =
  "flex items-center gap-1.5 rounded-[6px] border border-hairline/10 px-3 py-1.5 text-caption font-medium text-ink hover:bg-ink/5";

const THEME_OPTIONS: { value: ThemeMode; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

function AppearanceSection() {
  const theme = useApp((s) => s.theme);
  const gridCols = useApp((s) => s.gridCols);
  const showApproxBadge = useApp((s) => s.showApproxBadge);
  const reduceMotion = useApp((s) => s.reduceMotion);

  return (
    <div>
      <SectionHeading>Appearance</SectionHeading>

      <Row
        title="Theme"
        subtitle="Match system, or force light / dark"
        control={
          <Segmented
            options={THEME_OPTIONS}
            value={theme}
            onChange={(v) => useApp.getState().setTheme(v)}
          />
        }
      />

      <Row
        title="Grid density"
        subtitle="Default columns in the slide grid"
        control={
          <div className="flex items-center gap-2.5">
            <span className="text-caption text-subtle">3</span>
            <input
              type="range"
              min={3}
              max={10}
              step={1}
              value={gridCols}
              onChange={(e) => useApp.getState().setGridCols(Number(e.target.value))}
              className="h-1 w-[120px] cursor-pointer accent-[rgb(var(--accent-rgb))]"
              aria-label="Grid columns"
            />
            <span className="tabnum w-3 text-caption text-ink">{gridCols}</span>
          </div>
        }
      />

      <Row
        title="Show “Approximate” badge"
        subtitle="Flag previews that skipped unsupported constructs"
        control={
          <Switch
            checked={showApproxBadge}
            onToggle={() => useApp.getState().setShowApproxBadge(!showApproxBadge)}
          />
        }
      />

      <Row
        title="Reduce motion"
        subtitle="Minimize animations, on top of your system setting"
        divider={false}
        control={
          <Switch
            checked={reduceMotion}
            onToggle={() => useApp.getState().setReduceMotion(!reduceMotion)}
          />
        }
      />
    </div>
  );
}

function LibrarySection() {
  const roots = useApp((s) => s.roots);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());

  const toggleExcludes = (id: number) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div>
      <SectionHeading>Library</SectionHeading>

      <GroupLabel>Watched folders</GroupLabel>
      <div className="overflow-hidden rounded-[8px] border border-hairline/10">
        {roots.length === 0 && (
          <div className="px-3 py-3 text-caption text-subtle">
            No folders yet. Add one below to start indexing your decks.
          </div>
        )}
        {roots.map((r, i) => (
          <div key={r.id} className={cx(i < roots.length - 1 && "hairline-b")}>
            <div className="flex items-center gap-2.5 px-3 py-2.5">
              <Folder size={15} className="shrink-0 text-subtle" />
              <div className="min-w-0 flex-1 truncate text-body text-ink" title={r.path}>
                {r.path}
              </div>
              <span className="tabnum shrink-0 text-caption text-subtle">
                {r.deck_count} deck{r.deck_count === 1 ? "" : "s"}
              </span>
              <button
                onClick={() => toggleExcludes(r.id)}
                title="Edit exclude patterns"
                className={cx(
                  "flex shrink-0 rounded-[5px] p-1 hover:bg-ink/8 hover:text-ink",
                  expanded.has(r.id) ? "text-accent" : "text-subtle",
                )}
              >
                <SlidersHorizontal size={14} />
              </button>
              <button
                aria-label="Remove folder"
                onClick={() => void useApp.getState().removeRoot(r.id)}
                className="flex shrink-0 rounded-[5px] p-1 text-subtle hover:text-ink"
              >
                <X size={14} />
              </button>
            </div>
            {expanded.has(r.id) && (
              <div className="hairline-t px-1 pb-2 pt-1">
                <RootExcludesEditor root={r} />
              </div>
            )}
          </div>
        ))}
      </div>

      <div className="mt-5 flex items-center gap-2">
        <button onClick={() => void useApp.getState().addFolder()} className={outlineBtn}>
          <FolderPlus size={13} /> Add folder…
        </button>
        <span className="flex-1" />
        <button
          onClick={() => useApp.getState().confirmClearAndRebuild()}
          className={dangerGhost}
        >
          Clear &amp; rebuild index
        </button>
      </div>
    </div>
  );
}

/** Advanced: install the bundled `slideflow` CLI onto the user's PATH. */
function AdvancedSection() {
  const [installing, setInstalling] = useState<null | "system" | "user">(null);
  const codeChip = "rounded bg-ink/8 px-1 py-0.5 font-mono text-[11px] text-ink";

  const install = async (scope: "system" | "user") => {
    setInstalling(scope);
    try {
      const res = await api.installCli(scope);
      toast.success(res.note);
    } catch (err) {
      toast.error("Couldn't install the command line tool: " + String(err));
    } finally {
      setInstalling(null);
    }
  };

  return (
    <div>
      <SectionHeading>Advanced</SectionHeading>

      <GroupLabel>Command line tool</GroupLabel>
      <p className="text-body leading-relaxed text-subtle">
        Install the <code className={codeChip}>slideflow</code> command to index, search, and
        compose decks straight from your terminal. It links to the CLI bundled inside this app, so
        it stays in sync as Slideflow updates.
      </p>

      <div className="mt-4 flex gap-2">
        <button
          onClick={() => void install("system")}
          disabled={installing != null}
          className={cx(outlineBtn, "disabled:opacity-40")}
        >
          {installing === "system" ? (
            <Loader2 size={13} className="animate-spin" />
          ) : (
            <Terminal size={13} />
          )}
          Install system-wide
        </button>
        <button
          onClick={() => void install("user")}
          disabled={installing != null}
          className={cx(outlineBtn, "disabled:opacity-40")}
        >
          {installing === "user" ? (
            <Loader2 size={13} className="animate-spin" />
          ) : (
            <FolderPlus size={13} />
          )}
          Install for me only
        </button>
      </div>

      <p className="mt-2.5 text-caption text-subtle">
        System-wide links <code className={codeChip}>/usr/local/bin/slideflow</code> (may ask for
        your password). Just-for-me links <code className={codeChip}>~/.local/bin/slideflow</code>{" "}
        and adds it to your shell PATH. Then run <code className={codeChip}>slideflow --help</code>{" "}
        in a new terminal.
      </p>
    </div>
  );
}

function UpdatesSection() {
  const phase = useUpdater((s) => s.phase);
  const version = useUpdater((s) => s.version);
  const progress = useUpdater((s) => s.progress);
  const error = useUpdater((s) => s.error);
  const [auto, setAuto] = useState<boolean>(
    () => localStorage.getItem(AUTO_UPDATE_KEY) !== "0",
  );
  const [lastChecked, setLastChecked] = useState<string>(
    () => localStorage.getItem(LAST_CHECK_KEY) ?? "",
  );

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

  const checkNow = () => {
    const stamp = new Date().toLocaleString(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    });
    localStorage.setItem(LAST_CHECK_KEY, stamp);
    setLastChecked(stamp);
    void useUpdater.getState().check();
  };

  return (
    <div>
      <SectionHeading>Updates</SectionHeading>

      {phase !== "unsupported" && (
        <Row
          title="Automatic updates"
          subtitle="Check on launch and daily"
          control={<Switch checked={auto} onToggle={onToggle} />}
        />
      )}

      {phase === "ready" && (
        <div className="mt-4 flex items-center gap-3 rounded-[8px] bg-accent/[0.08] px-4 py-3.5">
          <Download size={20} className="shrink-0 text-accent" />
          <div className="min-w-0 flex-1">
            <div className="text-body font-semibold text-ink">
              Version {version} is ready to install
            </div>
            <div className="text-caption text-subtle">
              Downloaded and verified · restart to finish
            </div>
          </div>
          <button
            onClick={() => void useUpdater.getState().restart()}
            className="shrink-0 rounded-[6px] bg-accent px-3 py-1.5 text-caption font-semibold text-white hover:opacity-90"
          >
            Restart to update
          </button>
        </div>
      )}

      {phase === "downloading" && (
        <p className="tabnum mt-4 text-caption text-subtle">
          {progress != null
            ? `Downloading ${version} — ${Math.round(progress * 100)}%`
            : `Downloading ${version}…`}
        </p>
      )}
      {phase === "checking" && (
        <p className="mt-4 text-caption text-subtle">Checking for updates…</p>
      )}
      {phase === "installing" && (
        <p className="mt-4 text-caption text-subtle">Installing update…</p>
      )}
      {phase === "error" && error && (
        <p className="mt-4 text-caption text-red-500">{error}</p>
      )}
      {phase === "unsupported" && (
        <p className="mt-4 text-caption text-subtle">
          Updates are installed through your package manager.
        </p>
      )}

      {phase !== "unsupported" && (
        <div className="mt-4 flex items-center gap-2 text-caption text-subtle">
          <span>
            {lastChecked ? `Last checked ${lastChecked}` : "Checks automatically on launch"}
          </span>
          <span className="flex-1" />
          <button onClick={checkNow} className={outlineBtn}>
            Check now
          </button>
        </div>
      )}
    </div>
  );
}

const FONT_DOT: Record<FontFamily["status"], string> = {
  available: "var(--success)",
  downloadable: "var(--warning)",
  missing: "var(--danger)",
};

function fontStatusLabel(f: FontFamily): string {
  if (f.embedded) return "Embedded";
  if (f.status === "available") return "Available";
  if (f.status === "downloadable") return "Downloadable";
  return "Not installed";
}

/** Fonts: families the library references, each with an availability status.
 *  All fonts live under <app_data>/fonts and are never installed system-wide. */
function FontsSection() {
  const fonts = useFonts((s) => s.fonts);
  const loaded = useFonts((s) => s.loaded);
  const downloading = useFonts((s) => s.downloading);

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
      confirmIcon: <Download size={13} />,
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
      <SectionHeading aside="App-local · never installed system-wide">Fonts</SectionHeading>

      {loaded && fonts.length === 0 ? (
        <p className="text-caption text-subtle">
          No fonts detected yet. Add a folder of decks and scan to build the list.
        </p>
      ) : (
        <div className="overflow-hidden rounded-[8px] border border-hairline/10">
          {fonts.map((f, i) => (
            <FontRow
              key={f.family}
              font={f}
              divider={i < fonts.length - 1}
              downloading={downloading === f.family}
              onDownload={() => confirmDownload(f.family, f.download_source)}
              onRemove={() => confirmRemove(f.family)}
            />
          ))}
        </div>
      )}

      <div className="mt-4 flex gap-2">
        <button onClick={() => void useFonts.getState().addFonts()} className={outlineBtn}>
          <FolderPlus size={13} /> Add .ttf / .otf…
        </button>
        <button onClick={() => void useFonts.getState().revealFolder()} className={outlineBtn}>
          <FolderOpen size={13} /> Reveal folder
        </button>
      </div>
    </div>
  );
}

function FontRow({
  font,
  downloading,
  divider,
  onDownload,
  onRemove,
}: {
  font: FontFamily;
  downloading: boolean;
  divider: boolean;
  onDownload: () => void;
  onRemove: () => void;
}) {
  const dot = font.embedded ? "rgb(var(--accent-rgb))" : FONT_DOT[font.status];

  return (
    <div className={cx("flex items-center gap-2.5 px-3 py-2.5", divider && "hairline-b")}>
      <span
        className="h-2 w-2 shrink-0 rounded-full"
        style={{ background: dot }}
      />
      <div className="min-w-0 flex-1 truncate text-body text-ink">{font.family}</div>
      <span className="shrink-0 text-caption text-subtle">{fontStatusLabel(font)}</span>

      {font.status === "downloadable" &&
        (downloading ? (
          <button
            onClick={() => void useFonts.getState().cancelDownload()}
            className="shrink-0 rounded-[6px] px-1.5 py-0.5 text-caption text-subtle hover:bg-ink/8 hover:text-ink"
          >
            Cancel
          </button>
        ) : (
          <button
            onClick={onDownload}
            aria-label={`Download ${font.family}`}
            className="shrink-0 text-caption font-semibold text-accent hover:underline"
          >
            Download
          </button>
        ))}

      {font.status === "missing" && (
        <button
          onClick={() => void useFonts.getState().addFonts()}
          className="shrink-0 text-caption text-subtle hover:text-ink"
        >
          Add…
        </button>
      )}

      {font.removable && (
        <button
          onClick={onRemove}
          aria-label={`Remove ${font.family}`}
          className="flex shrink-0 rounded-[5px] p-1 text-subtle hover:text-ink"
        >
          <Trash2 size={14} />
        </button>
      )}
    </div>
  );
}

/** Semantic search: the on-device toggle, model download (with explicit
 *  consent — a ~490 MB one-time download), indexing progress, and management. */
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
        "This downloads a ≈490 MB multilingual model (multilingual-e5-small) from Hugging Face, sha256-pinned. It runs entirely on your Mac — nothing is ever uploaded.",
      confirmLabel: "Download",
      cancelLabel: "Not now",
      confirmIcon: <Download size={13} />,
      icon: <Sparkles size={18} />,
      onConfirm: () => void useSemantic.getState().download(),
      onCancel: () => void useSemantic.getState().setEnabled(false),
    });

  const onToggle = async () => {
    const sem = useSemantic.getState();
    if (enabled) {
      await sem.setEnabled(false);
      return;
    }
    await sem.setEnabled(true);
    if (useSemantic.getState().status?.state === "not_downloaded") {
      confirmDownload();
    }
  };

  const confirmDelete = () =>
    useApp.getState().requestConfirm({
      title: "Delete semantic search model?",
      message:
        "The ≈490 MB model and all slide embeddings will be removed. Lexical search keeps working. You can re-download anytime.",
      confirmLabel: "Delete model",
      destructive: true,
      onConfirm: () => void useSemantic.getState().deleteModel(),
    });

  const pct = downloadProgress != null ? Math.round(downloadProgress * 100) : null;
  const downloadingOrIndexing = state === "downloading" || indexing != null;

  return (
    <div>
      <SectionHeading
        aside={
          <>
            <Sparkles size={13} /> On-device
          </>
        }
      >
        Semantic search
      </SectionHeading>

      <Row
        title="Search by meaning"
        subtitle="Finds slides across languages · nothing leaves your Mac"
        divider={false}
        control={<Switch checked={enabled} onToggle={() => void onToggle()} />}
      />

      {state === "disabled" || state === "not_downloaded" ? (
        <p className="mt-4 text-body leading-relaxed text-subtle">
          Turn on to download a ≈490 MB multilingual model (multilingual-e5-small) from
          Hugging Face, sha256-pinned. German queries find English slides and vice-versa.
        </p>
      ) : null}

      {downloadingOrIndexing && (
        <div className="mt-4">
          <div className="mb-1.5 flex items-center gap-1.5 text-caption text-subtle">
            <Loader2 size={12} className="animate-spin" />
            {state === "downloading" ? (
              <span>Downloading model{pct != null ? ` — ${pct}%` : "…"}</span>
            ) : (
              <span>
                Indexing slides… <span className="tabnum">{indexing?.done}</span> of{" "}
                <span className="tabnum">{indexing?.total}</span>
              </span>
            )}
            {state === "downloading" && (
              <button
                onClick={() => void useSemantic.getState().cancelDownload()}
                className="ml-auto rounded-[6px] px-1.5 py-0.5 text-caption text-subtle hover:bg-ink/8 hover:text-ink"
              >
                Cancel
              </button>
            )}
          </div>
          <div className="h-1 overflow-hidden rounded-full bg-ink/10">
            <div
              className="h-full rounded-full bg-accent transition-[width] duration-300"
              style={{
                width:
                  state === "downloading"
                    ? pct != null
                      ? `${pct}%`
                      : "35%"
                    : indexing && indexing.total
                      ? `${Math.min(100, (indexing.done / indexing.total) * 100)}%`
                      : "35%",
              }}
            />
          </div>
          <p className="mt-2.5 text-caption text-subtle">
            Model downloaded · embeddings run locally in the background.
          </p>
        </div>
      )}

      {state === "error" && status?.error && (
        <div className="mt-4">
          <p className="text-caption text-red-500">{status.error}</p>
          <button
            onClick={() => void useSemantic.getState().download()}
            className={cx(outlineBtn, "mt-2")}
          >
            Retry download
          </button>
        </div>
      )}

      {state === "ready" && status && !indexing && (
        <div className="mt-4 rounded-[8px] border border-hairline/10 px-4 py-3.5">
          <div className="flex items-center gap-1.5 text-body font-semibold text-ink">
            <span
              className="h-2 w-2 rounded-full"
              style={{ background: "var(--success)" }}
            />
            Ready
          </div>
          <div className="tabnum mt-1 text-caption text-subtle">
            {status.model_id} · {status.embedded_slides} of {status.total_slides} slides
            indexed
          </div>
        </div>
      )}

      {state === "ready" && (
        <div className="mt-4 flex items-center gap-2">
          <button
            onClick={() => void useSemantic.getState().reindex()}
            disabled={indexing != null}
            className={cx(outlineBtn, "disabled:opacity-40")}
          >
            <RefreshCw size={13} /> Re-run indexing
          </button>
          <span className="flex-1" />
          <button onClick={confirmDelete} className={dangerGhost}>
            Delete model
          </button>
        </div>
      )}
    </div>
  );
}
