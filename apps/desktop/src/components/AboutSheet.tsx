import { useEffect, useState } from "react";
import { AnimatePresence, motion } from "framer-motion";
import { X, Coffee, Globe, Layers } from "lucide-react";
import { useApp } from "../stores/useApp";
import { prefersReducedMotion } from "../lib/utils";
import * as api from "../lib/api";

const WEBSITE_URL = "https://slideflow.app";
const COFFEE_URL = "https://www.buymeacoffee.com/michaelseliger";

/** About dialog: app identity, website link, and a "Buy me a coffee" button.
 *  Follows the ExportSheet overlay idiom (backdrop + spring card, reduced-motion
 *  aware). Links open in the default browser via the opener plugin. */
export default function AboutSheet() {
  const open = useApp((s) => s.aboutOpen);
  const reduce = prefersReducedMotion();
  const [version, setVersion] = useState<string>("");

  useEffect(() => {
    if (open) void api.getAppVersion().then(setVersion);
  }, [open]);

  const close = () => useApp.getState().setAboutOpen(false);

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
            className="w-full max-w-sm overflow-hidden rounded-[12px] bg-surface shadow-peek"
            initial={reduce ? false : { scale: 0.95, opacity: 0, y: 8 }}
            animate={{ scale: 1, opacity: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { scale: 0.97, opacity: 0 }}
            transition={{ type: "spring", stiffness: 320, damping: 30 }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-end px-3 py-2">
              <button
                onClick={close}
                className="rounded-full p-1 text-subtle hover:bg-ink/10"
                aria-label="Close"
              >
                <X size={16} />
              </button>
            </div>

            <div className="px-6 pb-6 pt-1 text-center">
              <div className="mx-auto mb-3 flex h-14 w-14 items-center justify-center rounded-[14px] bg-accent/[0.12]">
                <Layers size={28} className="text-accent" />
              </div>
              <div className="text-title font-semibold text-ink">Slideflow</div>
              <div className="tabnum mt-0.5 text-caption text-subtle">
                {version ? `Version ${version}` : " "}
              </div>
              <p className="mx-auto mt-3 max-w-[16rem] text-caption text-subtle">
                Search every slide across your decks and compose new ones — with
                every slide's original formatting intact.
              </p>

              <div className="mt-5 space-y-2">
                <button
                  onClick={() => void api.openUrl(WEBSITE_URL)}
                  className="flex w-full items-center justify-center gap-2 rounded-[8px] border border-hairline/10 px-4 py-2.5 text-body font-medium text-ink hover:bg-ink/5"
                >
                  <Globe size={16} /> slideflow.app
                </button>

                <button
                  onClick={() => void api.openUrl(COFFEE_URL)}
                  className="flex w-full items-center justify-center gap-2 rounded-[8px] border border-black bg-[#FFDD00] px-4 py-2.5 text-body font-semibold text-black hover:opacity-90"
                >
                  <Coffee size={16} /> Buy me a coffee
                </button>
              </div>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
