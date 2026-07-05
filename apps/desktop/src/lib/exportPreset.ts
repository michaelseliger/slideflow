// Frontend-only persistence for the export sheet's last-used settings.
// Deliberately NOT in lib/types.ts: there is no model.rs / Tauri counterpart.

/** Which output the export sheet produces. */
export type ExportFormat = "pptx" | "pdf" | "png";

/** Allowed PNG width presets (px); the longer edge follows the slide aspect. */
export const PNG_WIDTHS = [1280, 1920, 3840] as const;
export const DEFAULT_PNG_WIDTH = 1920;

/** Last-used export sheet settings (snake_case to match the pinned schema). */
export interface ExportPreset {
  title: string;
  include_notes: boolean;
  last_dir: string;
  /** Chosen output format (defaults to PowerPoint for pre-WS-D presets). */
  format: ExportFormat;
  /** Chosen PNG width preset. */
  png_width: number;
}

const EXPORT_PRESET_KEY = "slideflow.exportPreset.v1";

function isFormat(v: unknown): v is ExportFormat {
  return v === "pptx" || v === "pdf" || v === "png";
}

/** Read the saved preset, or null if missing/corrupt/storage-disabled. Fields
 *  added after the original schema (format, png_width) fall back to defaults so
 *  presets written by earlier builds still load. */
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
      return {
        title: p.title,
        include_notes: p.include_notes,
        last_dir: p.last_dir,
        format: isFormat(p.format) ? p.format : "pptx",
        png_width:
          typeof p.png_width === "number" &&
          (PNG_WIDTHS as readonly number[]).includes(p.png_width)
            ? p.png_width
            : DEFAULT_PNG_WIDTH,
      };
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
