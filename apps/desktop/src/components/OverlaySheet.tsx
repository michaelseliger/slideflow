import { AnimatePresence, motion } from "framer-motion";
import type { KeyboardEvent, ReactNode } from "react";
import { cx, prefersReducedMotion } from "../lib/utils";

interface OverlaySheetProps {
  open: boolean;
  /** Backdrop click / (default) Escape handler. */
  onClose: () => void;
  /** Card width + any extra card classes, e.g. "max-w-md" or "max-w-sm p-6". */
  cardClassName?: string;
  /** Backdrop stacking tier. Defaults to z-[95]; ConfirmDialog stacks above the
   *  other sheets at z-[96]. */
  zClassName?: string;
  /** Per-key handling while focus sits inside the card. The overlay always
   *  `stopPropagation()`s first, so grid keys can never leak to App.tsx's window
   *  listener behind the modal; this runs after. Defaults to Escape → onClose. */
  onCardKeyDown?: (e: KeyboardEvent<HTMLDivElement>) => void;
  children: ReactNode;
}

/**
 * Shared modal-overlay primitive: a fixed backdrop + spring card with
 * reduced-motion handling and a key trap. Extracted from the near-identical
 * blocks in SettingsSheet / ExportSheet / AboutSheet / ConfirmDialog so the
 * backdrop timing, spring, and (critically) the keydown trap live in one place.
 *
 * The card key trap only fires when focus is actually inside the card. Because
 * WKWebView leaves focus on <body> after a button click, sheets that must also
 * catch Escape with focus on body pair this with an App.tsx swallow branch
 * and/or their own window keydown listener.
 */
export default function OverlaySheet({
  open,
  onClose,
  cardClassName,
  zClassName = "z-[95]",
  onCardKeyDown,
  children,
}: OverlaySheetProps) {
  const reduce = prefersReducedMotion();

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className={cx(
            "fixed inset-0 flex items-center justify-center bg-black/40 p-8 backdrop-blur-xs",
            zClassName,
          )}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: reduce ? 0 : 0.14 }}
          onClick={onClose}
        >
          <motion.div
            className={cx(
              "w-full overflow-hidden rounded-[12px] bg-surface shadow-peek",
              cardClassName,
            )}
            initial={reduce ? false : { scale: 0.95, opacity: 0, y: 8 }}
            animate={{ scale: 1, opacity: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { scale: 0.97, opacity: 0 }}
            transition={{ type: "spring", stiffness: 320, damping: 30 }}
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => {
              // Keep grid keys from leaking to App.tsx's window listener while
              // the sheet is focused.
              e.stopPropagation();
              if (onCardKeyDown) onCardKeyDown(e);
              else if (e.key === "Escape") onClose();
            }}
          >
            {children}
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
