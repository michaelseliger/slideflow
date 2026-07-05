// Auto-update state, driven entirely by `update:event`s from the Rust side
// (`src-tauri/src/updates.rs` owns check → download → install; native builds
// also self-schedule checks — 5 s after boot, then daily). The store mirrors
// that lifecycle for the UI: a persistent "ready — restart" toast once per
// version, plus inline states for the About sheet's manual check.

import { create } from "zustand";
import * as api from "../lib/api";
import type { UpdateEvent } from "../lib/types";
import { useToast } from "./useToast";

export type UpdatePhase =
  | "idle" //         nothing to report (default resting state)
  | "unsupported" //  dev build or Linux deb/rpm install
  | "checking"
  | "upToDate" //     transient, only after a manual check
  | "downloading"
  | "ready" //        downloaded; waiting for restart (or install-on-quit)
  | "installing"
  | "error"; //       only surfaced for manual checks

interface UpdaterState {
  phase: UpdatePhase;
  /** Remote version, once one is known. */
  version: string | null;
  /** Download progress 0..1; null = indeterminate. */
  progress: number | null;
  error: string | null;
  /** Subscribe to update events (idempotent; called from App boot). */
  init: () => Promise<void>;
  /** Manual "Check for Updates…" — up-to-date/error render inline. */
  check: () => void;
  /** Install the downloaded update and relaunch. */
  restart: () => Promise<void>;
}

let initialized = false;
/** The current check was user-initiated, so quiet outcomes get surfaced. */
let manualCheck = false;
/** Versions already announced via toast — never nag twice for the same one. */
let notifiedVersion: string | null = null;
let upToDateTimer: number | undefined;

export const useUpdater = create<UpdaterState>((set, get) => ({
  phase: "idle",
  version: null,
  progress: null,
  error: null,

  init: async () => {
    if (initialized) return;
    initialized = true;
    if (!(await api.updatesSupported())) {
      set({ phase: "unsupported" });
      return;
    }
    await api.onUpdateEvent(handleEvent);
    // Native builds schedule their own silent checks in Rust; the browser
    // mock has no scheduler, so kick one off here to exercise the flow.
    if (!api.isTauri()) void api.checkForUpdates();
  },

  check: () => {
    const { phase } = get();
    if (phase === "unsupported" || phase === "downloading" || phase === "installing") {
      return;
    }
    manualCheck = true;
    set({ error: null });
    void api.checkForUpdates();
  },

  restart: async () => {
    if (get().phase !== "ready") return;
    set({ phase: "installing" });
    try {
      await api.restartToUpdate();
      // On success the process relaunches; nothing left to do here.
    } catch (err) {
      set({ phase: "error", error: String(err) });
    }
  },
}));

function handleEvent(ev: UpdateEvent) {
  const set = useUpdater.setState;
  window.clearTimeout(upToDateTimer);

  switch (ev.kind) {
    case "checking":
      set({ phase: "checking" });
      break;

    case "up_to_date":
      // Silent checks stay silent; a manual check confirms briefly, then the
      // About row returns to its resting state.
      if (manualCheck) {
        manualCheck = false;
        set({ phase: "upToDate" });
        upToDateTimer = window.setTimeout(() => {
          if (useUpdater.getState().phase === "upToDate") set({ phase: "idle" });
        }, 4000);
      } else {
        set({ phase: "idle" });
      }
      break;

    case "available":
      // The Rust flow rolls straight into the download.
      set({ phase: "downloading", version: ev.version, progress: null });
      break;

    case "downloading":
      set({
        phase: "downloading",
        progress: ev.total ? Math.min(ev.downloaded / ev.total, 1) : null,
      });
      break;

    case "ready":
      manualCheck = false;
      set({ phase: "ready", version: ev.version, progress: 1 });
      if (notifiedVersion !== ev.version) {
        notifiedVersion = ev.version;
        useToast.getState().push(
          {
            kind: "info",
            message: `Slideflow ${ev.version} is ready — restart to update.`,
            action: {
              label: "Restart",
              run: () => void useUpdater.getState().restart(),
            },
          },
          0, // persistent until dismissed
        );
      }
      break;

    case "error":
      // Background checks fail quietly (offline is normal); manual ones
      // surface the message inline in the About sheet.
      if (manualCheck) {
        manualCheck = false;
        set({ phase: "error", error: ev.message });
      } else {
        console.warn("[updater]", ev.message);
        set({ phase: "idle" });
      }
      break;
  }
}
