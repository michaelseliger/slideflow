import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion } from "framer-motion";
import confetti from "canvas-confetti";
import {
  X,
  FolderOpen,
  ExternalLink,
  Loader2,
  CheckCircle2,
  ShieldCheck,
} from "lucide-react";
import { useApp } from "../stores/useApp";
import { useTray } from "../stores/useTray";
import { toast } from "../stores/useToast";
import { dirname, prefersReducedMotion } from "../lib/utils";
import { readExportPreset, writeExportPreset } from "../lib/exportPreset";
import { trayHasAspectMismatch } from "../lib/trayDims";
import type { ComposeReport, FitMode } from "../lib/types";
import * as api from "../lib/api";

type Phase =
  | { step: "form" }
  | { step: "working"; done: number; total: number }
  | { step: "done"; report: ComposeReport }
  | { step: "error"; message: string };

/** Export sheet: name + location → determinate per-slide progress → success
 *  with the single delight moment (confetti, reduced-motion aware). */
export default function ExportSheet() {
  const open = useApp((s) => s.exportOpen);
  const items = useTray((s) => s.items);
  const reduce = prefersReducedMotion();

  // Only when the tray mixes aspect ratios is the fit ambiguous enough to ask.
  const hasAspectMismatch = trayHasAspectMismatch(items);

  const [title, setTitle] = useState("Slideflow Deck");
  const [includeNotes, setIncludeNotes] = useState(false);
  const [fitMode, setFitMode] = useState<FitMode>("ensure_fit");
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
      setPresetApplied(true);
    } else {
      setPresetApplied(false);
    }
  }, [open]);

  const close = () => {
    if (phase.step === "working") return; // don't cancel mid-assembly
    useApp.getState().setExportOpen(false);
  };

  const runExport = async () => {
    const picks = useTray.getState().picks();
    if (picks.length === 0) return;
    const safe = title.trim() || "Slideflow Deck";
    const defaultName = `${safe.replace(/[^\w.-]+/g, "-")}.pptx`;
    const lastDir = readExportPreset()?.last_dir;
    const outputPath = await api.pickSavePath(defaultName, lastDir);
    if (!outputPath) return;

    const total = picks.length;
    setPhase({ step: "working", done: 0, total });

    // Optimistic per-slide progress while the (atomic) compose runs, so the bar
    // is determinate and lively rather than an indeterminate spinner.
    let tick = 0;
    const timer = window.setInterval(() => {
      tick = Math.min(total - 1, tick + 1);
      setPhase({ step: "working", done: tick, total });
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
      // Remember what was actually used (folder only — never the filename).
      writeExportPreset({
        title: safe,
        include_notes: includeNotes,
        last_dir: dirname(report.output_path),
      });
      setPhase({ step: "done", report });
      // Reflect this export in the "Most exported" sort without a rescan.
      void useApp.getState().refreshExportCounts();
    } catch (err) {
      window.clearInterval(timer);
      setPhase({ step: "error", message: String(err) });
      toast.error(`Export failed: ${String(err)}`);
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

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-[95] flex items-center justify-center bg-black/40 p-8 backdrop-blur-sm"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: reduce ? 0 : 0.14 }}
          onClick={close}
        >
          <motion.div
            className="w-full max-w-md overflow-hidden rounded-[12px] bg-surface shadow-peek"
            initial={reduce ? false : { scale: 0.95, opacity: 0, y: 8 }}
            animate={{ scale: 1, opacity: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { scale: 0.97, opacity: 0 }}
            transition={{ type: "spring", stiffness: 320, damping: 30 }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between px-5 py-3 hairline-b">
              <h2 className="text-title font-semibold text-ink">Export deck</h2>
              {phase.step !== "working" && (
                <button
                  onClick={close}
                  className="rounded-full p-1 text-subtle hover:bg-ink/10"
                >
                  <X size={16} />
                </button>
              )}
            </div>

            <div className="p-5">
              {phase.step === "form" && (
                <>
                  <label className="mb-3 block">
                    <span className="mb-1 block text-caption font-medium text-subtle">
                      Deck title
                    </span>
                    <input
                      autoFocus
                      value={title}
                      onChange={(e) => setTitle(e.target.value)}
                      onKeyDown={(e) => e.key === "Enter" && void runExport()}
                      className="selectable w-full rounded-[6px] border border-hairline/10 bg-canvas px-2.5 py-2 text-body text-ink outline-none focus:border-accent"
                    />
                  </label>

                  {presetApplied && (
                    <p className="mb-3 -mt-1 text-caption text-subtle">
                      Starting from your last export.
                    </p>
                  )}

                  <label className="mb-4 flex items-center gap-2 text-body text-ink">
                    <input
                      type="checkbox"
                      checked={includeNotes}
                      onChange={(e) => setIncludeNotes(e.target.checked)}
                      className="h-4 w-4 accent-[rgb(var(--accent-rgb))]"
                    />
                    Include speaker notes
                  </label>

                  {hasAspectMismatch && (
                    <fieldset className="mb-4">
                      <legend className="mb-1.5 text-caption font-medium text-subtle">
                        Mixed slide sizes
                      </legend>
                      <label className="mb-1.5 flex items-start gap-2 text-body text-ink">
                        <input
                          type="radio"
                          name="fit-mode"
                          checked={fitMode === "ensure_fit"}
                          onChange={() => setFitMode("ensure_fit")}
                          className="mt-1 h-3.5 w-3.5 accent-[rgb(var(--accent-rgb))]"
                        />
                        <span>
                          Ensure fit
                          <span className="text-subtle">
                            {" "}
                            — scale to fit, letterbox
                          </span>
                        </span>
                      </label>
                      <label className="flex items-start gap-2 text-body text-ink">
                        <input
                          type="radio"
                          name="fit-mode"
                          checked={fitMode === "maximize"}
                          onChange={() => setFitMode("maximize")}
                          className="mt-1 h-3.5 w-3.5 accent-[rgb(var(--accent-rgb))]"
                        />
                        <span>
                          Maximize
                          <span className="text-subtle">
                            {" "}
                            — fill the slide, may crop
                          </span>
                        </span>
                      </label>
                    </fieldset>
                  )}

                  <div className="mb-4 flex items-start gap-2 rounded-[6px] bg-accent/[0.08] p-2.5 text-caption text-subtle">
                    <ShieldCheck size={15} className="mt-0.5 shrink-0 text-accent" />
                    <span>
                      Every slide keeps its <b className="text-ink">original theme,
                      master, and formatting</b>. Slideflow never re-themes or
                      reflows your slides on export.
                    </span>
                  </div>

                  <div className="flex items-center justify-between">
                    <span className="tabnum text-caption text-subtle">
                      {items.length} slide{items.length === 1 ? "" : "s"}
                    </span>
                    <div className="flex gap-2">
                      <button
                        onClick={close}
                        className="rounded-[6px] px-3 py-2 text-body text-subtle hover:bg-ink/5"
                      >
                        Cancel
                      </button>
                      <button
                        onClick={() => void runExport()}
                        disabled={items.length === 0}
                        className="rounded-[6px] bg-accent px-4 py-2 text-body font-semibold text-white hover:opacity-90 disabled:opacity-40"
                      >
                        Choose location…
                      </button>
                    </div>
                  </div>
                </>
              )}

              {phase.step === "working" && (
                <div className="py-2">
                  <div className="flex items-center gap-2 text-body text-ink">
                    <Loader2 size={16} className="animate-spin text-accent" />
                    Assembling your deck…
                  </div>
                  <div className="tabnum mt-1 text-caption text-subtle">
                    Slide {phase.done + 1} of {phase.total} — preserving original
                    formatting
                  </div>
                  <div className="mt-3 h-2 overflow-hidden rounded-full bg-ink/10">
                    <motion.div
                      className="h-full rounded-full bg-accent"
                      animate={{
                        width: `${((phase.done + 1) / phase.total) * 100}%`,
                      }}
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
                    className="mx-auto mb-2 w-fit"
                  >
                    <CheckCircle2 size={44} className="text-green-500" />
                  </motion.div>
                  <div className="text-title font-semibold text-ink">
                    Your deck is ready
                  </div>
                  <div className="tabnum mt-1 text-caption text-subtle">
                    {phase.report.slides_written} slides from{" "}
                    {phase.report.source_decks} deck
                    {phase.report.source_decks === 1 ? "" : "s"}
                  </div>
                  {phase.report.warnings.length > 0 && (
                    <div className="mt-2 rounded-[6px] bg-amber-500/10 p-2 text-left text-caption text-amber-600">
                      {phase.report.warnings.map((w, i) => (
                        <div key={i}>{w}</div>
                      ))}
                    </div>
                  )}
                  {phase.report.notes.length > 0 && (
                    <div className="mt-2 rounded-[6px] bg-ink/5 p-2 text-left text-caption text-subtle">
                      {phase.report.notes.map((n, i) => (
                        <div key={i}>{n}</div>
                      ))}
                    </div>
                  )}
                  <div className="mt-4 flex justify-center gap-2">
                    <button
                      onClick={() =>
                        void api.revealInFinder(phase.report.output_path)
                      }
                      className="flex items-center gap-1.5 rounded-[6px] border border-hairline/10 px-3 py-2 text-body text-ink hover:bg-ink/5"
                    >
                      <FolderOpen size={15} /> Reveal in Finder
                    </button>
                    <button
                      onClick={() => void api.openFile(phase.report.output_path)}
                      className="flex items-center gap-1.5 rounded-[6px] bg-accent px-3 py-2 text-body font-medium text-white hover:opacity-90"
                    >
                      <ExternalLink size={15} /> Open
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
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
