import { useEffect, useRef, useState } from "react";
import { motion } from "framer-motion";
import confetti from "canvas-confetti";
import {
  X,
  FolderOpen,
  ExternalLink,
  Loader2,
  Check,
  Download,
} from "lucide-react";
import { useApp } from "../stores/useApp";
import { useTray } from "../stores/useTray";
import { toast } from "../stores/useToast";
import { dirname, prefersReducedMotion } from "../lib/utils";
import OverlaySheet from "./OverlaySheet";
import {
  readExportPreset,
  writeExportPreset,
  PNG_WIDTHS,
  DEFAULT_PNG_WIDTH,
} from "../lib/exportPreset";
import type { ExportFormat } from "../lib/exportPreset";
import { trayHasAspectMismatch } from "../lib/trayDims";
import type { ExportReport, FitMode } from "../lib/types";
import * as api from "../lib/api";

/** What the success screen needs, independent of which format produced it. */
interface DoneResult {
  format: ExportFormat;
  /** Path to reveal in Finder (the file, or the folder for PNGs). */
  revealPath: string;
  /** Path to open in the default app, or null (PNG folder — reveal only). */
  openPath: string | null;
  /** One-line summary, e.g. "12 slides from 3 decks" or "12 PNG images". */
  summary: string;
  warnings: string[];
  /** Neutral, informational notes (pptx only — e.g. a scaled deck). */
  notes: string[];
}

type Phase =
  | { step: "form" }
  | { step: "working"; done: number; total: number; format: ExportFormat }
  | { step: "done"; result: DoneResult }
  | { step: "error"; message: string };

const FORMAT_LABELS: Record<ExportFormat, string> = {
  pptx: "PowerPoint",
  pdf: "PDF",
  png: "PNG images",
};

/** Export sheet: pick a format + location → determinate per-slide progress →
 *  success with the single delight moment (confetti, reduced-motion aware). */
export default function ExportSheet() {
  const open = useApp((s) => s.exportOpen);
  const items = useTray((s) => s.items);
  const reduce = prefersReducedMotion();

  // Only when the tray mixes aspect ratios is the fit ambiguous enough to ask.
  const hasAspectMismatch = trayHasAspectMismatch(items);

  const [title, setTitle] = useState("Slideflow Deck");
  const [includeNotes, setIncludeNotes] = useState(false);
  const [fitMode, setFitMode] = useState<FitMode>("ensure_fit");
  const [format, setFormat] = useState<ExportFormat>("pptx");
  const [pngWidth, setPngWidth] = useState<number>(DEFAULT_PNG_WIDTH);
  const [phase, setPhase] = useState<Phase>({ step: "form" });
  const [presetApplied, setPresetApplied] = useState(false);
  const confettiFired = useRef(false);

  useEffect(() => {
    if (!open) return;
    setPhase({ step: "form" });
    confettiFired.current = false;
    // Re-apply the last SUCCESSFUL export's preset each time the sheet opens.
    const p = readExportPreset();
    if (p) {
      setTitle(p.title);
      setIncludeNotes(p.include_notes);
      setFormat(p.format);
      setPngWidth(p.png_width);
      setPresetApplied(true);
    } else {
      setPresetApplied(false);
    }
  }, [open]);

  const close = () => {
    if (phase.step === "working") return; // don't cancel mid-export
    useApp.getState().setExportOpen(false);
  };

  // WKWebView leaves focus on <body> after a button click, so the card's own
  // onKeyDown can't be relied on to catch Escape. Register a capture-phase
  // window listener that closes on Escape (guarded so it never cancels a running
  // export) and stops the event before it reaches App.tsx's global handler —
  // otherwise Escape would clear the search query behind the sheet.
  const phaseRef = useRef(phase);
  phaseRef.current = phase;
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      if (phaseRef.current.step !== "working") {
        useApp.getState().setExportOpen(false);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open]);

  // --- PowerPoint: style-preserving composition (optimistic progress) --------
  const runPptx = async (safe: string, outputPath: string) => {
    const picks = useTray.getState().picks();
    const total = picks.length;
    setPhase({ step: "working", done: 0, total, format: "pptx" });

    // Optimistic per-slide progress while the (atomic) compose runs, so the bar
    // is determinate and lively rather than an indeterminate spinner.
    let tick = 0;
    const timer = window.setInterval(() => {
      tick = Math.min(total - 1, tick + 1);
      setPhase({ step: "working", done: tick, total, format: "pptx" });
    }, Math.max(60, 500 / total));

    try {
      // Send the fit mode only when a mixed-aspect tray actually surfaced the
      // choice; a same-aspect tray leaves it unset (the engine auto-scales).
      const report = await api.composeDeck(
        picks,
        outputPath,
        safe,
        includeNotes,
        hasAspectMismatch ? fitMode : undefined,
      );
      window.clearInterval(timer);
      writeExportPreset({
        title: safe,
        include_notes: includeNotes,
        last_dir: dirname(report.output_path),
        format: "pptx",
        png_width: pngWidth,
      });
      setPhase({
        step: "done",
        result: {
          format: "pptx",
          revealPath: report.output_path,
          openPath: report.output_path,
          summary: `${report.slides_written} slide${report.slides_written === 1 ? "" : "s"} from ${report.source_decks} deck${report.source_decks === 1 ? "" : "s"}`,
          warnings: report.warnings,
          notes: report.notes,
        },
      });
      void useApp.getState().refreshExportCounts();
    } catch (err) {
      window.clearInterval(timer);
      setPhase({ step: "error", message: String(err) });
      toast.error(`Export failed: ${String(err)}`);
    }
  };

  // --- PDF / PNG: rendered export with REAL per-slide progress ---------------
  const runRendered = async (
    fmt: "pdf" | "png",
    safe: string,
    target: string,
    call: () => Promise<ExportReport>,
  ) => {
    const picks = useTray.getState().picks();
    const total = picks.length;
    setPhase({ step: "working", done: 0, total, format: fmt });

    // Real per-slide progress streamed from the engine over `export:event`.
    const unlisten = await api.onExportEvent((ev) => {
      setPhase({ step: "working", done: ev.done, total: ev.total, format: fmt });
    });

    try {
      const report = await call();
      unlisten();
      const isPng = fmt === "png";
      const firstFile = report.files_written[0] ?? null;
      const lastDir = isPng ? target : dirname(firstFile ?? target);
      writeExportPreset({
        title: safe,
        include_notes: includeNotes,
        last_dir: lastDir,
        format: fmt,
        png_width: pngWidth,
      });
      const count = report.files_written.length;
      setPhase({
        step: "done",
        result: {
          format: fmt,
          // PNG: reveal the folder via its first file so Finder shows them all.
          revealPath: firstFile ?? target,
          openPath: isPng ? null : firstFile,
          summary: isPng
            ? `${count} PNG image${count === 1 ? "" : "s"}`
            : `${total} slide${total === 1 ? "" : "s"} → PDF`,
          warnings: report.warnings,
          notes: [],
        },
      });
      void useApp.getState().refreshExportCounts();
    } catch (err) {
      unlisten();
      setPhase({ step: "error", message: String(err) });
      toast.error(`Export failed: ${String(err)}`);
    }
  };

  const runExport = async () => {
    const picks = useTray.getState().picks();
    if (picks.length === 0) return;
    const safe = title.trim() || "Slideflow Deck";
    const sanitized = safe.replace(/[^\w.-]+/g, "-");
    const lastDir = readExportPreset()?.last_dir;

    if (format === "pptx") {
      const outputPath = await api.pickSavePath(`${sanitized}.pptx`, lastDir);
      if (!outputPath) return;
      await runPptx(safe, outputPath);
    } else if (format === "pdf") {
      const outputPath = await api.pickSavePath(`${sanitized}.pdf`, lastDir, {
        name: "PDF",
        extensions: ["pdf"],
      });
      if (!outputPath) return;
      await runRendered("pdf", safe, outputPath, () =>
        api.exportTrayPdf(picks, outputPath, safe),
      );
    } else {
      const dir = await api.pickFolder();
      if (!dir) return;
      await runRendered("png", safe, dir, () =>
        api.exportTrayPngs(picks, dir, pngWidth),
      );
    }
  };

  // The one success moment.
  useEffect(() => {
    if (phase.step !== "done" || confettiFired.current) return;
    confettiFired.current = true;
    if (reduce) return;
    const end = Date.now() + 700;
    (function frame() {
      confetti({
        particleCount: 3,
        angle: 60,
        spread: 55,
        origin: { x: 0.5, y: 0.5 },
        colors: ["#0A84FF", "#30D158", "#FF9F0A", "#BF5AF2"],
        disableForReducedMotion: true,
      });
      if (Date.now() < end) requestAnimationFrame(frame);
    })();
  }, [phase, reduce]);

  // Footer preview of what will be written: a filename for pptx/pdf, a file
  // count for the PNG folder export.
  const safeName = (title.trim() || "Slideflow Deck").replace(/[^\w.-]+/g, "-");
  const previewFilename =
    format === "png"
      ? `${items.length} file${items.length === 1 ? "" : "s"} · ${pngWidth}px`
      : `${safeName}.${format}`;

  return (
    <OverlaySheet open={open} onClose={close} cardClassName="max-w-md">
      <div className="flex items-center justify-between px-5 py-4 hairline-b">
              <h2 className="text-title font-semibold text-ink">
                Export {items.length} slide{items.length === 1 ? "" : "s"}
              </h2>
              {phase.step !== "working" && (
                <button
                  onClick={close}
                  className="rounded-full p-1 text-subtle hover:bg-ink/10"
                >
                  <X size={16} />
                </button>
              )}
            </div>

            {phase.step === "form" && (
              <>
                {/* Format tabs (underline). */}
                <div className="flex gap-5 px-5 hairline-b">
                  {(["pptx", "pdf", "png"] as ExportFormat[]).map((f) => (
                    <button
                      key={f}
                      onClick={() => setFormat(f)}
                      aria-pressed={format === f}
                      className={`py-3.5 text-body transition-colors ${
                        format === f
                          ? "font-semibold text-accent shadow-[inset_0_-2px_0_rgb(var(--accent-rgb))]"
                          : "font-medium text-subtle hover:text-ink"
                      }`}
                    >
                      {FORMAT_LABELS[f]}
                    </button>
                  ))}
                </div>

                <div className="p-5">
                  {/* Fidelity note — honest about what each format preserves. */}
                  {format === "pptx" ? (
                    <p className="text-body leading-relaxed text-subtle">
                      Fidelity-preserving — each slide keeps its layout, master and
                      theme. Shared parts are deduplicated automatically.
                    </p>
                  ) : format === "pdf" ? (
                    <p className="text-body leading-relaxed text-subtle">
                      One PDF, rendered by Slideflow&rsquo;s preview engine. Text stays
                      selectable and searchable.
                    </p>
                  ) : (
                    <p className="text-body leading-relaxed text-subtle">
                      One PNG per slide, exported to a folder.
                    </p>
                  )}

                  {/* Deck title — names the single .pptx/.pdf artifact. PNG
                      export writes one file per slide (named from its source
                      deck), so a deck title has no meaning there. */}
                  {format !== "png" && (
                    <label className="mt-4 block">
                      <span className="mb-1 block text-caption font-medium text-subtle">
                        Deck title
                      </span>
                      <input
                        value={title}
                        onChange={(e) => setTitle(e.target.value)}
                        onKeyDown={(e) => e.key === "Enter" && void runExport()}
                        className="selectable w-full rounded-[7px] border border-hairline/10 bg-canvas px-2.5 py-2 text-body text-ink outline-none focus:border-accent"
                      />
                    </label>
                  )}

                  {/* Mixed-aspect fit choice — only affects PowerPoint composition
                      (PDF/PNG render each slide at its own size). */}
                  {format === "pptx" && hasAspectMismatch && (
                    <OptionRow
                      title="Fit mode"
                      sublabel="Your picks mix 4:3 and 16:9 slides"
                    >
                      <Segmented
                        options={[
                          { value: "ensure_fit", label: "Ensure fit" },
                          { value: "maximize", label: "Maximize" },
                        ]}
                        value={fitMode}
                        onChange={(v) => setFitMode(v as FitMode)}
                      />
                    </OptionRow>
                  )}

                  {/* Page size is fixed to each slide's dimensions — plain info,
                      not a control, since there's nothing to choose. */}
                  {format === "pdf" && (
                    <p className="mt-4 text-caption text-subtle">
                      Each page is sized to its slide — no reflow or letterboxing.
                    </p>
                  )}

                  {/* PNG width preset. */}
                  {format === "png" && (
                    <OptionRow title="Width" sublabel="pixels · aspect preserved">
                      <Segmented
                        options={PNG_WIDTHS.map((w) => ({
                          value: String(w),
                          label: String(w),
                        }))}
                        value={String(pngWidth)}
                        onChange={(v) => setPngWidth(Number(v))}
                      />
                    </OptionRow>
                  )}

                  {/* Include notes — PowerPoint only (rendered exports have no notes
                      surface). */}
                  {format === "pptx" && (
                    <div className="mt-4 flex items-center hairline-t pt-3.5">
                      <div className="flex-1 text-body font-medium text-ink">
                        Include speaker notes
                      </div>
                      <Toggle on={includeNotes} onClick={() => setIncludeNotes((v) => !v)} />
                    </div>
                  )}

                  {presetApplied && (
                    <p className="mt-3 text-caption text-subtle">
                      Starting from your last export.
                    </p>
                  )}
                </div>

                <div className="flex items-center gap-2.5 px-5 py-3.5 hairline-t">
                  <span className="tabnum min-w-0 truncate text-caption text-subtle">
                    {previewFilename}
                  </span>
                  <span className="flex-1" />
                  <button
                    onClick={close}
                    className="rounded-[8px] px-3.5 py-1.5 text-body font-medium text-ink hover:bg-ink/5"
                  >
                    Cancel
                  </button>
                  <button
                    onClick={() => void runExport()}
                    disabled={items.length === 0}
                    className="flex items-center gap-1.5 rounded-[8px] bg-accent px-3.5 py-1.5 text-body font-semibold text-white hover:opacity-90 disabled:opacity-40"
                  >
                    <Download size={13} /> Export…
                  </button>
                </div>
              </>
            )}

            {phase.step !== "form" && (
              <div className="p-5">
              {phase.step === "working" && (
                <div className="py-2">
                  <div className="flex items-center gap-2 text-body text-ink">
                    <Loader2 size={16} className="animate-spin text-accent" />
                    {phase.format === "pptx"
                      ? "Assembling your deck…"
                      : phase.format === "pdf"
                        ? "Rendering your PDF…"
                        : "Rendering your images…"}
                  </div>
                  <div className="tabnum mt-1 text-caption text-subtle">
                    Slide {Math.min(phase.done + 1, phase.total)} of {phase.total}
                    {phase.format === "pptx" ? " — preserving original formatting" : ""}
                  </div>
                  <div className="mt-3 h-2 overflow-hidden rounded-full bg-ink/10">
                    <motion.div
                      className="h-full rounded-full bg-accent"
                      animate={{ width: `${(phase.done / phase.total) * 100}%` }}
                      transition={{ ease: "easeOut", duration: 0.2 }}
                    />
                  </div>
                </div>
              )}

              {phase.step === "done" && (
                <div className="py-2 text-center">
                  <motion.div
                    initial={reduce ? false : { scale: 0.5, opacity: 0 }}
                    animate={{ scale: 1, opacity: 1 }}
                    transition={{ type: "spring", stiffness: 300, damping: 18 }}
                    className="mx-auto mb-4 flex h-[58px] w-[58px] items-center justify-center rounded-full"
                    style={{ background: "rgb(34 197 94 / 0.14)", color: "var(--success)" }}
                  >
                    <Check size={30} />
                  </motion.div>
                  <div className="text-heading font-semibold text-ink">
                    {phase.result.format === "png"
                      ? "Your images are ready"
                      : "Your deck is ready"}
                  </div>
                  <div className="tabnum mt-1.5 text-caption text-subtle">
                    {phase.result.summary}
                  </div>
                  {phase.result.warnings.length > 0 && (
                    <div className="mt-2 rounded-[6px] bg-amber-500/10 p-2 text-left text-caption text-amber-600">
                      {phase.result.warnings.map((w, i) => (
                        <div key={i}>{w}</div>
                      ))}
                    </div>
                  )}
                  {phase.result.notes.length > 0 && (
                    <div className="mt-2 rounded-[6px] bg-ink/5 p-2 text-left text-caption text-subtle">
                      {phase.result.notes.map((n, i) => (
                        <div key={i}>{n}</div>
                      ))}
                    </div>
                  )}
                  <div className="mt-5 flex flex-wrap justify-center gap-2">
                    <button
                      onClick={() => void api.revealInFinder(phase.result.revealPath)}
                      className="flex items-center gap-1.5 rounded-[8px] border border-hairline/10 px-3.5 py-1.5 text-body text-ink hover:bg-ink/5"
                    >
                      <FolderOpen size={13} /> Reveal in Finder
                    </button>
                    {phase.result.openPath && (
                      <button
                        onClick={() => void api.openFile(phase.result.openPath!)}
                        className="flex items-center gap-1.5 rounded-[8px] border border-hairline/10 px-3.5 py-1.5 text-body text-ink hover:bg-ink/5"
                      >
                        <ExternalLink size={13} /> Open
                      </button>
                    )}
                    <button
                      onClick={close}
                      className="rounded-[8px] bg-accent px-3.5 py-1.5 text-body font-medium text-white hover:opacity-90"
                    >
                      Done
                    </button>
                  </div>
                </div>
              )}

              {phase.step === "error" && (
                <div className="py-2">
                  <div className="text-body font-medium text-red-500">
                    Export failed
                  </div>
                  <div className="mt-1 text-caption text-subtle">
                    {phase.message}
                  </div>
                  <div className="mt-4 flex justify-end gap-2">
                    <button
                      onClick={close}
                      className="rounded-[6px] px-3 py-2 text-body text-subtle hover:bg-ink/5"
                    >
                      Close
                    </button>
                    <button
                      onClick={() => setPhase({ step: "form" })}
                      className="rounded-[6px] bg-accent px-4 py-2 text-body font-medium text-white"
                    >
                      Try again
                    </button>
                  </div>
                </div>
              )}
              </div>
            )}
    </OverlaySheet>
  );
}

/** A settings-style row: title + optional sublabel on the left, a control on the
 *  right. Used for the export sheet's fit-mode / width / page-size options. */
function OptionRow({
  title,
  sublabel,
  children,
}: {
  title: string;
  sublabel?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="mt-4 flex items-start gap-3">
      <div className="flex-1">
        <div className="text-body font-medium text-ink">{title}</div>
        {sublabel && <div className="mt-0.5 text-caption text-subtle">{sublabel}</div>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

/** A compact segmented control (bordered, accent-filled active segment). */
function Segmented({
  options,
  value,
  onChange,
}: {
  options: { value: string; label: string }[];
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <div className="inline-flex overflow-hidden rounded-[6px] border border-hairline/[0.12]">
      {options.map((o) => (
        <button
          key={o.value}
          onClick={() => onChange(o.value)}
          className={`px-3 py-1.5 text-caption transition-colors ${
            o.value === value
              ? "bg-accent font-semibold text-white"
              : "font-medium text-subtle hover:bg-ink/5"
          }`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** An iOS-style toggle switch. */
function Toggle({ on, onClick }: { on: boolean; onClick: () => void }) {
  return (
    <button
      role="switch"
      aria-checked={on}
      onClick={onClick}
      className="relative inline-block h-[22px] w-[38px] shrink-0 rounded-full transition-colors"
      style={{ background: on ? "rgb(var(--accent-rgb))" : "rgb(var(--ink-rgb) / 0.18)" }}
    >
      <span
        className="absolute top-0.5 h-[18px] w-[18px] rounded-full bg-white shadow transition-[left]"
        style={{ left: on ? 18 : 2 }}
      />
    </button>
  );
}
