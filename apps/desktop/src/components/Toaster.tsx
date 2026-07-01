import { AnimatePresence, motion } from "framer-motion";
import { CheckCircle2, Info, AlertCircle, X } from "lucide-react";
import { useToast } from "../stores/useToast";
import { prefersReducedMotion } from "../lib/utils";

/** Bottom-center toast stack. Copy personality lives here (never in errors). */
export default function Toaster() {
  const toasts = useToast((s) => s.toasts);
  const dismiss = useToast((s) => s.dismiss);
  const reduce = prefersReducedMotion();

  return (
    <div className="pointer-events-none fixed inset-x-0 bottom-4 z-[110] flex flex-col items-center gap-2">
      <AnimatePresence initial={false}>
        {toasts.map((t) => (
          <motion.div
            key={t.id}
            layout={!reduce}
            initial={reduce ? { opacity: 0 } : { opacity: 0, y: 16, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, y: 8, scale: 0.96 }}
            transition={{ type: "spring", stiffness: 400, damping: 30 }}
            className="pointer-events-auto flex items-center gap-2.5 rounded-[10px] bg-elevated px-3.5 py-2.5 text-body text-ink shadow-peek ring-1 ring-black/5"
          >
            {t.kind === "success" && (
              <CheckCircle2 size={16} className="text-green-500" />
            )}
            {t.kind === "info" && <Info size={16} className="text-accent" />}
            {t.kind === "error" && (
              <AlertCircle size={16} className="text-red-500" />
            )}
            <span>{t.message}</span>
            {t.action && (
              <button
                onClick={() => {
                  t.action!.run();
                  dismiss(t.id);
                }}
                className="ml-1 rounded-[5px] bg-accent/[0.14] px-2 py-0.5 text-caption font-medium text-accent hover:bg-accent/20"
              >
                {t.action.label}
              </button>
            )}
            <button
              onClick={() => dismiss(t.id)}
              className="ml-1 rounded-full p-0.5 text-subtle hover:bg-ink/10"
            >
              <X size={13} />
            </button>
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
