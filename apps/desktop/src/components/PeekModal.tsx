import { AnimatePresence, motion } from "framer-motion";
import { X, ChevronLeft, ChevronRight, Plus, FolderOpen, Check } from "lucide-react";
import { useApp } from "../stores/useApp";
import { useTray } from "../stores/useTray";
import { toast } from "../stores/useToast";
import { useSlidePreview } from "../lib/useSlideSvg";
import { deckDisplayName, prefersReducedMotion } from "../lib/utils";
import * as api from "../lib/api";
import ApproxBadge from "./ApproxBadge";

/** Finder-style Quick Look: a large centered preview with arrow-key navigation
 *  (wired globally in App) and speaker notes. Space/Esc dismiss. */
export default function PeekModal() {
  const peekIndex = useApp((s) => s.peekIndex);
  const results = useApp((s) => s.results);
  const reduce = prefersReducedMotion();
  const hit = peekIndex != null ? results[peekIndex] : null;
  const { src: previewSrc, dropped } = useSlidePreview(hit?.slide.id ?? null, "full");
  const inTray = useTray((s) =>
    hit ? s.items.some((i) => i.uid === `${hit.slide.deck_id}:${hit.slide.slide_index}`) : false,
  );

  return (
    <AnimatePresence>
      {hit && (
        <motion.div
          className="fixed inset-0 z-[90] flex items-center justify-center bg-black/55 p-10 backdrop-blur-sm"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: reduce ? 0 : 0.14 }}
          onClick={() => useApp.getState().closePeek()}
        >
          <button
            className="absolute right-4 top-4 rounded-full bg-white/10 p-2 text-white/80 hover:bg-white/20"
            onClick={() => useApp.getState().closePeek()}
            title="Close (esc)"
          >
            <X size={18} />
          </button>

          {peekIndex! > 0 && (
            <NavBtn side="left" onClick={() => useApp.getState().peekBy(-1)} />
          )}
          {peekIndex! < results.length - 1 && (
            <NavBtn side="right" onClick={() => useApp.getState().peekBy(1)} />
          )}

          <motion.div
            className="flex max-h-full w-full max-w-5xl flex-col overflow-hidden rounded-[12px] bg-surface shadow-peek"
            initial={reduce ? false : { scale: 0.94, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            exit={reduce ? { opacity: 0 } : { scale: 0.96, opacity: 0 }}
            transition={{ type: "spring", stiffness: 300, damping: 30 }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between gap-3 px-4 py-2.5 hairline-b">
              <div className="min-w-0">
                <div className="truncate text-title font-semibold text-ink">
                  {hit.slide.title || deckDisplayName(hit.deck)}
                </div>
                <div className="tabnum truncate text-caption text-subtle" title={hit.deck.path}>
                  {deckDisplayName(hit.deck)} · Slide {hit.slide.slide_index} of{" "}
                  {hit.deck.slide_count}
                </div>
                {dropped.length > 0 && (
                  <div className="mt-1">
                    <ApproxBadge dropped={dropped} variant="peek" />
                  </div>
                )}
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <button
                  onClick={() => void api.openFile(hit.deck.path)}
                  className="flex items-center gap-1.5 rounded-[6px] border border-hairline/10 px-2.5 py-1.5 text-caption text-ink hover:bg-ink/5"
                >
                  <FolderOpen size={13} /> Open source deck
                </button>
                <button
                  disabled={inTray}
                  onClick={() => {
                    const added = useTray
                      .getState()
                      .add([{ slide: hit.slide, deck: hit.deck }]);
                    if (added > 0) toast.success("Added to the tray");
                  }}
                  className="flex items-center gap-1.5 rounded-[6px] bg-accent px-3 py-1.5 text-caption font-medium text-white disabled:opacity-50"
                >
                  {inTray ? <Check size={13} /> : <Plus size={13} />}
                  {inTray ? "In tray" : "Add to tray"}
                </button>
              </div>
            </div>

            <div className="min-h-0 flex-1 overflow-auto bg-canvas p-5">
              {previewSrc ? (
                <img
                  src={previewSrc}
                  alt={hit.slide.title || "Slide preview"}
                  draggable={false}
                  decoding="async"
                  className="mx-auto block w-full max-w-4xl overflow-hidden rounded-[8px] bg-white object-contain shadow-tile"
                  style={{ aspectRatio: "16 / 9" }}
                />
              ) : (
                <div
                  className="shimmer mx-auto w-full max-w-4xl overflow-hidden rounded-[8px]"
                  style={{ aspectRatio: "16 / 9" }}
                />
              )}

              {hit.slide.notes && (
                <div className="mx-auto mt-4 max-w-4xl">
                  <div className="mb-1 text-caption font-semibold uppercase tracking-wide text-subtle/70">
                    Speaker notes
                  </div>
                  <div className="selectable whitespace-pre-wrap rounded-[8px] bg-ink/5 p-3 text-body leading-relaxed text-subtle">
                    {hit.slide.notes}
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

function NavBtn({ side, onClick }: { side: "left" | "right"; onClick: () => void }) {
  return (
    <button
      className={`absolute top-1/2 -translate-y-1/2 rounded-full bg-white/10 p-2.5 text-white/80 hover:bg-white/20 ${
        side === "left" ? "left-4" : "right-4"
      }`}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      title={side === "left" ? "Previous (←)" : "Next (→)"}
    >
      {side === "left" ? <ChevronLeft size={20} /> : <ChevronRight size={20} />}
    </button>
  );
}
