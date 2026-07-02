// Minimal hand-rolled toast store. No dependency, respects the brief's rule
// that transient copy personality lives here but never in errors/dialogs.

import { create } from "zustand";

export type ToastKind = "info" | "success" | "error";

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
  /** Optional action button (e.g. Undo). */
  action?: { label: string; run: () => void };
}

interface ToastState {
  toasts: Toast[];
  push: (t: Omit<Toast, "id">, ttlMs?: number) => number;
  dismiss: (id: number) => void;
}

let nextId = 1;

export const useToast = create<ToastState>((set, get) => ({
  toasts: [],
  push: (t, ttlMs = 3200) => {
    const id = nextId++;
    set((s) => ({ toasts: [...s.toasts, { ...t, id }] }));
    if (ttlMs > 0) {
      window.setTimeout(() => get().dismiss(id), ttlMs);
    }
    return id;
  },
  dismiss: (id) =>
    set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}));

// Convenience helpers.
export const toast = {
  info: (message: string, action?: Toast["action"]) =>
    useToast.getState().push({ kind: "info", message, action }),
  success: (message: string, action?: Toast["action"]) =>
    useToast.getState().push({ kind: "success", message, action }),
  error: (message: string) =>
    useToast.getState().push({ kind: "error", message }, 5000),
};
