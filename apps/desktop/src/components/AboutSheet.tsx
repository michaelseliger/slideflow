import { useEffect, useState, type ReactNode } from "react";
import { X, Coffee } from "lucide-react";
import { useApp } from "../stores/useApp";
import { useUpdater } from "../stores/useUpdater";
import * as api from "../lib/api";
import OverlaySheet from "./OverlaySheet";

const WEBSITE_URL = "https://slideflow.app";
const COFFEE_URL = "https://www.buymeacoffee.com/michaelseliger";
const REPO_URL = "https://github.com/michaelseliger/slideflow";
const RELEASES_URL = "https://github.com/michaelseliger/slideflow/releases";
const ISSUES_URL = "https://github.com/michaelseliger/slideflow/issues/new";

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
      <button
        onClick={() => void useUpdater.getState().restart()}
        title="Restart to install the update"
        className="mt-3 inline-flex items-center gap-1.5 rounded-full bg-accent/[0.12] px-3 py-1 text-caption font-semibold text-accent hover:bg-accent/[0.18]"
      >
        Version {version} ready · restart to update
      </button>
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
  const [version, setVersion] = useState<string>("");

  useEffect(() => {
    if (open) void api.getAppVersion().then(setVersion);
  }, [open]);

  const close = () => useApp.getState().setAboutOpen(false);

  return (
    <OverlaySheet open={open} onClose={close} cardClassName="max-w-sm">
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
              <AppMark />
              <div className="text-[20px] font-bold leading-tight tracking-tight text-ink">
                Slideflow
              </div>
              <div className="tabnum mt-0.5 text-caption text-subtle">
                {version ? `Version ${version}` : " "}
              </div>
              <UpdateStatus />

              <div className="mt-4 flex flex-wrap items-center justify-center gap-x-3.5 gap-y-1 text-caption">
                <AboutLink onClick={() => void api.openUrl(WEBSITE_URL)}>Website</AboutLink>
                <span className="text-hairline/20">·</span>
                <AboutLink onClick={() => void api.openUrl(RELEASES_URL)}>
                  What's new
                </AboutLink>
                <span className="text-hairline/20">·</span>
                <AboutLink onClick={() => void api.openUrl(REPO_URL)}>GitHub</AboutLink>
                <span className="text-hairline/20">·</span>
                <AboutLink onClick={() => void api.openUrl(ISSUES_URL)}>
                  Report a bug
                </AboutLink>
              </div>

              <div className="mt-5 hairline-t pt-5">
                <button
                  onClick={() => void api.openUrl(COFFEE_URL)}
                  className="inline-flex items-center gap-2 rounded-[8px] border border-hairline/[0.14] px-4 py-2 text-body font-medium text-ink hover:bg-ink/5"
                >
                  <Coffee size={16} /> Buy me a coffee
                </button>
              </div>

              <p className="mt-3.5 text-caption text-subtle/80">
                MIT License · local-first · no telemetry
              </p>
            </div>
    </OverlaySheet>
  );
}

/** The Slideflow app mark: three stacked, fanned slides on a warm off-white
 *  squircle — matches the icon shipped in `src-tauri/icons`. */
function AppMark() {
  return (
    <div
      className="relative mx-auto mb-4 h-[66px] w-[66px] overflow-hidden rounded-[19px] shadow-tile"
      style={{ background: "#F3F1EC" }}
      aria-hidden
    >
      <span
        className="absolute h-[23px] w-[34px] rounded-[4px]"
        style={{ left: 16, top: 15, background: "var(--brand-beige)", transform: "rotate(-9deg)" }}
      />
      <span
        className="absolute h-[23px] w-[34px] rounded-[4px]"
        style={{ left: 16, top: 21, background: "var(--brand-gray)", transform: "rotate(-2deg)" }}
      />
      <span
        className="absolute h-[23px] w-[34px] rounded-[4px]"
        style={{ left: 16, top: 27, background: "var(--brand-blue)", transform: "rotate(6deg)" }}
      />
    </div>
  );
}

/** A plain accent text link in the About footer row. */
function AboutLink({
  children,
  onClick,
}: {
  children: ReactNode;
  onClick: () => void;
}) {
  return (
    <button onClick={onClick} className="text-accent hover:underline">
      {children}
    </button>
  );
}
