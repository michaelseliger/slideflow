import { AnimatePresence, motion } from "framer-motion";
import { ChevronLeft, ChevronRight, Plus, FolderOpen, Check } from "lucide-react";
import { useApp } from "../stores/useApp";
import { useTray, uidFor } from "../stores/useTray";
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
  const showApproxBadge = useApp((s) => s.showApproxBadge);
  const reduce = prefersReducedMotion();
  const hit = peekIndex != null ? results[peekIndex] : null;
  const { src: previewSrc, dropped } = useSlidePreview(hit?.slide.id ?? null, "full");
  const inTray = useTray((s) =>
    hit ? s.items.some((i) => i.uid === uidFor(hit.deck, hit.slide)) : false,
  );

  return (
    <AnimatePresence>
      {hit && (
        <motion.div
          className="fixed inset-0 z-[90] flex items-center justify-center bg-black/55 p-10 backdrop-blur-xs"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: reduce ? 0 : 0.14 }}
          onClick={() => useApp.getState().closePeek()}
        >
          <motion.div
            className="flex max-h-full w-full max-w-[820px] flex-col overflow-hidden rounded-[12px] bg-elevated shadow-peek"
            initial={reduce ? false : { scale: 0.94, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            exit={reduce ? { opacity: 0 } : { scale: 0.96, opacity: 0 }}
            transition={{ type: "spring", stiffness: 300, damping: 30 }}
            onClick={(e) => e.stopPropagation()}
          >
            {/* Preview, with in-frame prev/next controls. */}
            <div className="relative shrink-0 bg-white" style={{ aspectRatio: "16 / 9" }}>
              {previewSrc ? (
                <img
                  src={previewSrc}
                  alt={hit.slide.title || "Slide preview"}
                  draggable={false}
                  decoding="async"
                  className="h-full w-full object-contain"
                />
              ) : (
                <div className="shimmer h-full w-full" />
              )}
              {peekIndex! > 0 && (
                <NavBtn side="left" onClick={() => useApp.getState().peekBy(-1)} />
              )}
              {peekIndex! < results.length - 1 && (
                <NavBtn side="right" onClick={() => useApp.getState().peekBy(1)} />
              )}
              {showApproxBadge && dropped.length > 0 && (
                <div className="absolute left-3.5 top-3.5">
                  <ApproxBadge dropped={dropped} variant="peek" />
                </div>
              )}
            </div>

            {/* Footer: identity + notes on the left, actions stacked on the right. */}
            <div className="flex items-start gap-5 px-5 py-4">
              <div className="min-w-0 flex-1">
                <div className="truncate text-heading font-semibold text-ink">
                  {hit.slide.title || deckDisplayName(hit.deck)}
                </div>
                <div className="tabnum mt-0.5 truncate text-body text-subtle" title={hit.deck.path}>
                  {deckDisplayName(hit.deck)} · Slide {hit.slide.slide_index} of{" "}
                  {hit.deck.slide_count}
                </div>
                {hit.slide.notes && (
                  <div className="selectable mt-3 whitespace-pre-wrap text-body leading-relaxed text-ink">
                    <span className="text-subtle">Notes · </span>
                    {hit.slide.notes}
                  </div>
                )}
              </div>
              <div className="flex w-[170px] shrink-0 flex-col gap-2">
                <button
                  disabled={inTray}
                  onClick={() => {
                    const added = useTray
                      .getState()
                      .add([{ slide: hit.slide, deck: hit.deck }]);
                    if (added > 0) toast.success("Added to the tray");
                  }}
                  className="flex h-7 items-center justify-center gap-1.5 rounded-[6px] bg-accent text-body font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
                >
                  {inTray ? <Check size={13} /> : <Plus size={13} />}
                  {inTray ? "In tray" : "Add to tray"}
                </button>
                <button
                  onClick={() => void api.openFile(hit.deck.path)}
                  className="flex h-7 items-center justify-center gap-1.5 rounded-[6px] border border-hairline/10 text-body text-ink transition-colors hover:bg-ink/5"
                >
                  <FolderOpen size={13} /> Open source deck
                </button>
              </div>
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
      className={`absolute top-1/2 flex h-9 w-9 -translate-y-1/2 items-center justify-center rounded-full bg-black/50 text-white hover:bg-black/65 ${
        side === "left" ? "left-3.5" : "right-3.5"
      }`}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      title={side === "left" ? "Previous (←)" : "Next (→)"}
    >
      {side === "left" ? <ChevronLeft size={18} /> : <ChevronRight size={18} />}
    </button>
  );
}
