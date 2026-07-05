// App-local font state, driven by `list_library_fonts` snapshots plus the
// `font:download` and `fonts:changed` streams from the Rust side
// (`src-tauri/src/fonts.rs` owns the fonts dir, the curated download resolver,
// and harvesting). Mirrors the `useSemantic` pattern.

import { create } from "zustand";
import * as api from "../lib/api";
import type { FontDownloadEvent, FontFamily } from "../lib/types";
import { toast } from "./useToast";
import { clearSlideSvgCache } from "../lib/useSlideSvg";
import { useApp } from "./useApp";

interface FontsStore {
  /** The library's font inventory with availability; empty until first fetch. */
  fonts: FontFamily[];
  /** `<app_data>/fonts` path, for the "Reveal in Finder" affordance. */
  dir: string;
  /** True while (re)fetching the list. */
  loading: boolean;
  /** The family currently downloading, or null. */
  downloading: string | null;
  /** Whether the first fetch has resolved (drives the empty/rescan hint). */
  loaded: boolean;

  /** Subscribe to events + fetch the list (idempotent; called at app boot). */
  init: () => Promise<void>;
  refresh: () => Promise<void>;
  addFonts: () => Promise<void>;
  remove: (family: string) => Promise<void>;
  download: (family: string) => Promise<void>;
  cancelDownload: () => Promise<void>;
  revealFolder: () => Promise<void>;
}

let initialized = false;

export const useFonts = create<FontsStore>((set, get) => ({
  fonts: [],
  dir: "",
  loading: false,
  downloading: null,
  loaded: false,

  init: async () => {
    if (initialized) return;
    initialized = true;
    await api.onFontDownloadEvent(handleDownloadEvent);
    await api.onFontsChanged(() => {
      // Any font-set change (add/remove/download/harvest): the host already
      // wiped the on-disk preview cache, so drop the session SVG cache too and
      // re-render, then refresh the list.
      clearSlideSvgCache();
      void useApp.getState().refresh();
      void get().refresh();
    });
    try {
      set({ dir: await api.fontsDir() });
    } catch {
      /* browser mock without a dir — ignore */
    }
    await get().refresh();
  },

  refresh: async () => {
    set({ loading: true });
    try {
      const fonts = await api.listLibraryFonts();
      set({ fonts, loaded: true });
    } catch (err) {
      console.warn("[fonts] list failed:", err);
    } finally {
      set({ loading: false });
    }
  },

  addFonts: async () => {
    try {
      const paths = await api.pickFontFiles();
      if (paths.length === 0) return;
      const fonts = await api.addUserFonts(paths);
      set({ fonts });
      toast.success(paths.length === 1 ? "Font added" : `${paths.length} fonts added`);
    } catch (err) {
      toast.error(`Couldn't add fonts: ${String(err)}`);
    }
  },

  remove: async (family) => {
    try {
      const fonts = await api.removeAppFont(family);
      set({ fonts });
      toast.info(`Removed ${family}`);
    } catch (err) {
      toast.error(`Couldn't remove ${family}: ${String(err)}`);
    }
  },

  download: async (family) => {
    set({ downloading: family });
    try {
      const started = await api.downloadFont(family);
      if (!started) {
        set({ downloading: null });
        toast.info("A font download is already running.");
      }
    } catch (err) {
      set({ downloading: null });
      toast.error(`Couldn't download ${family}: ${String(err)}`);
    }
  },

  cancelDownload: async () => {
    try {
      await api.cancelFontDownload();
    } catch (err) {
      toast.error(`Couldn't cancel the download: ${String(err)}`);
    }
  },

  revealFolder: async () => {
    const dir = get().dir;
    if (!dir) return;
    try {
      await api.revealInFinder(dir);
    } catch (err) {
      toast.error(`Couldn't open the fonts folder: ${String(err)}`);
    }
  },
}));

function handleDownloadEvent(ev: FontDownloadEvent) {
  const set = useFonts.setState;
  switch (ev.kind) {
    case "started":
      set({ downloading: ev.family });
      break;
    case "done":
      set({ downloading: null });
      toast.success(`${ev.family} downloaded`);
      void useFonts.getState().refresh();
      break;
    case "canceled":
      set({ downloading: null });
      void useFonts.getState().refresh();
      break;
    case "error":
      set({ downloading: null });
      toast.error(`Couldn't download ${ev.family}: ${ev.message}`);
      void useFonts.getState().refresh();
      break;
  }
}
