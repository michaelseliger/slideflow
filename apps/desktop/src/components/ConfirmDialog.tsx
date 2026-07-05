import { AnimatePresence, motion } from "framer-motion";
import { useApp } from "../stores/useApp";
import { prefersReducedMotion } from "../lib/utils";

/** Reusable confirm dialog driven by `useApp.confirm`. Follows the AboutSheet
 *  overlay idiom (backdrop + spring card, reduced-motion aware). Sits at z-[96],
 *  a tier above the other overlays (z-[95]), so it can appear over them. */
export default function ConfirmDialog() {
  const confirm = useApp((s) => s.confirm);
  const reduce = prefersReducedMotion();

  // Declining (cancel button, backdrop, Escape) also fires the config's
  // onCancel hook, so consent flows can actively revert state on "no".
  const cancel = () => {
    const cfg = useApp.getState().confirm;
    useApp.getState().dismissConfirm();
    void cfg?.onCancel?.();
  };
  const run = async () => {
    const cfg = useApp.getState().confirm;
    useApp.getState().dismissConfirm();
    await cfg?.onConfirm();
  };

  return (
    <AnimatePresence>
      {confirm && (
        <motion.div
          className="fixed inset-0 z-[96] flex items-center justify-center bg-black/40 p-8 backdrop-blur-sm"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: reduce ? 0 : 0.14 }}
          onClick={cancel}
        >
          <motion.div
            className="w-full max-w-sm overflow-hidden rounded-[12px] bg-surface p-6 shadow-peek"
            initial={reduce ? false : { scale: 0.95, opacity: 0, y: 8 }}
            animate={{ scale: 1, opacity: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { scale: 0.97, opacity: 0 }}
            transition={{ type: "spring", stiffness: 320, damping: 30 }}
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => {
              // Keep grid keys from leaking to App.tsx's window listener while
              // the dialog is focused; Escape cancels.
              e.stopPropagation();
              if (e.key === "Escape") cancel();
            }}
          >
            <h2 className="text-title font-semibold text-ink">{confirm.title}</h2>
            <p className="mt-2 text-body text-subtle">{confirm.message}</p>

            <div className="mt-5 flex justify-end gap-2">
              <button
                autoFocus
                onClick={cancel}
                className="rounded-[8px] px-4 py-2 text-body font-medium text-ink hover:bg-ink/5"
              >
                {confirm.cancelLabel ?? "Cancel"}
              </button>
              <button
                onClick={() => void run()}
                className={`rounded-[8px] px-4 py-2 text-body font-medium text-white hover:opacity-90 ${
                  confirm.destructive ? "bg-red-500" : "bg-accent"
                }`}
              >
                {confirm.confirmLabel}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
