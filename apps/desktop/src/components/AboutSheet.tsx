import { useEffect, useState } from "react";
import { AnimatePresence, motion } from "framer-motion";
import { X, Coffee, Globe, Layers } from "lucide-react";
import { useApp } from "../stores/useApp";
import { useUpdater } from "../stores/useUpdater";
import { prefersReducedMotion } from "../lib/utils";
import * as api from "../lib/api";

const WEBSITE_URL = "https://slideflow.app";
const COFFEE_URL = "https://www.buymeacoffee.com/michaelseliger";
const RELEASE_URL = "https://github.com/michaelseliger/slideflow/releases/tag/v";

/** Inline update status under the version line. States mirror `useUpdater`;
 *  release notes deliberately link to the GitHub release page (the notes baked
 *  into latest.json are frozen at CI time, before the draft body is written). */
export function UpdateStatus() {
  const phase = useUpdater((s) => s.phase);
  const version = useUpdater((s) => s.version);
  const progress = useUpdater((s) => s.progress);
  const error = useUpdater((s) => s.error);

  if (phase === "unsupported") {
    // Only real Linux deb/rpm installs land here in production; dev builds
    // (also unsupported) show nothing.
    if (api.isTauri() && !import.meta.env.DEV) {
      return (
        <p className="mt-2 text-caption text-subtle">
          Updates are installed through your package manager.
        </p>
      );
    }
    return null;
  }

  if (phase === "checking") {
    return <p className="mt-2 text-caption text-subtle">Checking for updates…</p>;
  }

  if (phase === "upToDate") {
    return <p className="mt-2 text-caption text-subtle">You're up to date.</p>;
  }

  if (phase === "downloading") {
    return (
      <p className="tabnum mt-2 text-caption text-subtle">
        {progress != null
          ? `Downloading ${version} — ${Math.round(progress * 100)}%`
          : `Downloading ${version}…`}
      </p>
    );
  }

  if (phase === "installing") {
    return <p className="mt-2 text-caption text-subtle">Installing update…</p>;
  }

  if (phase === "ready") {
    return (
      <div className="mt-3 space-y-1.5">
        <button
          onClick={() => void useUpdater.getState().restart()}
          className="w-full rounded-[8px] bg-accent px-4 py-2 text-body font-medium text-white hover:opacity-90"
        >
          Restart to Update
        </button>
        <button
          onClick={() => void api.openUrl(`${RELEASE_URL}${version}`)}
          className="text-caption text-accent hover:underline"
        >
          See what's new in {version}
        </button>
      </div>
    );
  }

  if (phase === "error") {
    return (
      <div className="mt-2 space-y-0.5">
        <p className="mx-auto max-w-[16rem] text-caption text-subtle">{error}</p>
        <button
          onClick={() => useUpdater.getState().check()}
          className="text-caption font-medium text-accent hover:underline"
        >
          Try Again
        </button>
      </div>
    );
  }

  // idle
  return (
    <button
      onClick={() => useUpdater.getState().check()}
      className="mt-2 text-caption font-medium text-accent hover:underline"
    >
      Check for Updates…
    </button>
  );
}

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
            onKeyDown={(e) => {
              // Trap keys so grid shortcuts and Escape don't leak to App.tsx's
              // window listener behind the modal (mirrors ConfirmDialog);
              // Escape closes.
              e.stopPropagation();
              if (e.key === "Escape") close();
            }}
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
              <UpdateStatus />
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
