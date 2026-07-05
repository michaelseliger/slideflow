// Semantic-search state, driven by `get_embedding_status` snapshots plus the
// `model:download` and `embed:event` streams from the Rust side
// (`src-tauri/src/semantic.rs` owns the download, the enable preference, the
// embedder bootstrap, and the backfill). Mirrors the `useUpdater` pattern.

import { create } from "zustand";
import * as api from "../lib/api";
import type { EmbedEvent, EmbeddingStatus, ModelDownloadEvent } from "../lib/types";
import { toast } from "./useToast";

interface SemanticStore {
  /** Latest backend snapshot; null until the first fetch resolves. */
  status: EmbeddingStatus | null;
  /** Model download progress 0..1; null when no download is running. */
  downloadProgress: number | null;
  /** Embedding backfill progress; null when idle. */
  indexing: { done: number; total: number } | null;

  /** Subscribe to events + fetch the initial status (idempotent; App boot). */
  init: () => Promise<void>;
  /** Re-fetch the status snapshot. */
  refresh: () => Promise<void>;
  /** Whether semantic ranking is usable right now. */
  isReady: () => boolean;

  setEnabled: (enabled: boolean) => Promise<void>;
  download: () => Promise<void>;
  cancelDownload: () => Promise<void>;
  reindex: () => Promise<void>;
  deleteModel: () => Promise<void>;
}

let initialized = false;

export const useSemantic = create<SemanticStore>((set, get) => ({
  status: null,
  downloadProgress: null,
  indexing: null,

  init: async () => {
    if (initialized) return;
    initialized = true;
    await api.onModelDownloadEvent(handleDownloadEvent);
    await api.onEmbedEvent(handleEmbedEvent);
    await get().refresh();
  },

  refresh: async () => {
    try {
      const status = await api.getEmbeddingStatus();
      set({ status });
    } catch (err) {
      console.warn("[semantic] status fetch failed:", err);
    }
  },

  isReady: () => get().status?.state === "ready",

  setEnabled: async (enabled) => {
    try {
      await api.setSemanticSearchEnabled(enabled);
    } catch (err) {
      toast.error(`Couldn't update semantic search: ${String(err)}`);
    }
    await get().refresh();
  },

  download: async () => {
    set({ downloadProgress: 0 });
    try {
      await api.downloadEmbeddingModel();
    } catch (err) {
      set({ downloadProgress: null });
      toast.error(`Couldn't start the model download: ${String(err)}`);
    }
    await get().refresh();
  },

  cancelDownload: async () => {
    try {
      await api.cancelModelDownload();
    } catch (err) {
      toast.error(`Couldn't cancel the download: ${String(err)}`);
    }
  },

  reindex: async () => {
    try {
      const started = await api.startEmbedBackfill();
      if (!started) toast.info("Indexing is already running (or the model isn't loaded yet).");
    } catch (err) {
      toast.error(`Couldn't start indexing: ${String(err)}`);
    }
  },

  deleteModel: async () => {
    try {
      await api.deleteEmbeddingModel();
      toast.info("Semantic search model deleted");
    } catch (err) {
      toast.error(`Couldn't delete the model: ${String(err)}`);
    }
    set({ downloadProgress: null, indexing: null });
    await get().refresh();
  },
}));

function handleDownloadEvent(ev: ModelDownloadEvent) {
  const set = useSemantic.setState;
  const refresh = () => void useSemantic.getState().refresh();

  switch (ev.kind) {
    case "progress":
      set({
        downloadProgress:
          ev.overall_total > 0 ? Math.min(ev.overall_downloaded / ev.overall_total, 1) : null,
      });
      break;
    case "done":
      set({ downloadProgress: null });
      toast.success("Semantic search model downloaded — indexing your slides now.");
      refresh();
      break;
    case "canceled":
      set({ downloadProgress: null });
      refresh();
      break;
    case "error":
      set({ downloadProgress: null });
      toast.error(`Model download failed: ${ev.message}`);
      refresh();
      break;
  }
}

// Throttle the mid-backfill status re-fetches: progress events fire once per
// embedding batch, and each refresh runs two cheap COUNTs over the library. One
// fetch per ~500 ms keeps the "X of Y slides indexed" line honestly live without
// hammering IPC on a large first-time index.
let lastStatusRefresh = 0;
function refreshStatusThrottled() {
  const now = Date.now();
  if (now - lastStatusRefresh < 500) return;
  lastStatusRefresh = now;
  void useSemantic.getState().refresh();
}

function handleEmbedEvent(ev: EmbedEvent) {
  const set = useSemantic.setState;

  switch (ev.kind) {
    case "started":
      set({ indexing: { done: 0, total: ev.total } });
      // Pull the real embedded/total counts now — otherwise the summary line
      // stays frozen at whatever it was before the backfill (e.g. "0 of 0" when
      // semantic search was enabled on an empty library, then slides scanned).
      lastStatusRefresh = 0;
      refreshStatusThrottled();
      break;
    case "progress":
      set({ indexing: { done: ev.done, total: ev.total } });
      refreshStatusThrottled();
      break;
    case "finished":
      set({ indexing: null });
      void useSemantic.getState().refresh();
      break;
    case "error":
      set({ indexing: null });
      toast.error(`Slide indexing failed: ${ev.message}`);
      void useSemantic.getState().refresh();
      break;
  }
}
