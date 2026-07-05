// Frontend-only persistence for the export sheet's last-used settings.
// Deliberately NOT in lib/types.ts: there is no model.rs / Tauri counterpart.

/** Last-used export sheet settings (snake_case to match the pinned schema). */
export interface ExportPreset {
  title: string;
  include_notes: boolean;
  last_dir: string;
}

const EXPORT_PRESET_KEY = "slideflow.exportPreset.v1";

/** Read the saved preset, or null if missing/corrupt/storage-disabled. */
export function readExportPreset(): ExportPreset | null {
  try {
    const raw = localStorage.getItem(EXPORT_PRESET_KEY);
    if (raw === null) return null;
    const p = JSON.parse(raw) as Partial<ExportPreset>;
    if (
      typeof p.title === "string" &&
      typeof p.include_notes === "boolean" &&
      typeof p.last_dir === "string"
    ) {
      return { title: p.title, include_notes: p.include_notes, last_dir: p.last_dir };
    }
    return null;
  } catch {
    return null;
  }
}

/** Persist the preset; silently swallow storage errors (quota/private mode). */
export function writeExportPreset(p: ExportPreset): void {
  try {
    localStorage.setItem(EXPORT_PRESET_KEY, JSON.stringify(p));
  } catch {
    // Storage may be unavailable; the preset is a best-effort nicety.
  }
}
