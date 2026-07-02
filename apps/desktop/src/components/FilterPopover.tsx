import { useEffect, useRef } from "react";
import { useApp } from "../stores/useApp";
import { basename } from "../lib/utils";

/** Lightweight filter popover: deck name, source folder, and a modified-date
 *  range. Changes flow straight into the store's `filters` and re-run search. */
export default function FilterPopover({ onClose }: { onClose: () => void }) {
  const filters = useApp((s) => s.filters);
  const roots = useApp((s) => s.roots);
  const setFilters = useApp((s) => s.setFilters);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [onClose]);

  const toISODate = (unix?: number | null) =>
    unix ? new Date(unix * 1000).toISOString().slice(0, 10) : "";
  const fromISO = (v: string) =>
    v ? Math.floor(new Date(v + "T00:00:00").getTime() / 1000) : null;

  return (
    <div
      ref={ref}
      className="absolute right-0 top-full z-50 mt-1.5 w-72 rounded-[8px] border border-hairline/10 bg-elevated p-3 text-body shadow-peek"
      onClick={(e) => e.stopPropagation()}
    >
      <Field label="Deck name / file">
        <input
          className="selectable w-full rounded-[6px] border border-hairline/10 bg-canvas px-2 py-1.5 text-body text-ink outline-none focus:border-accent"
          placeholder="e.g. Roadmap"
          value={filters.deck_query ?? ""}
          onChange={(e) =>
            setFilters({ deck_query: e.target.value || null })
          }
        />
      </Field>

      <Field label="Source folder">
        <select
          className="w-full rounded-[6px] border border-hairline/10 bg-canvas px-2 py-1.5 text-body text-ink outline-none focus:border-accent"
          value={filters.path_prefix ?? ""}
          onChange={(e) =>
            setFilters({ path_prefix: e.target.value || null })
          }
        >
          <option value="">Any folder</option>
          {roots.map((r) => (
            <option key={r.id} value={r.path}>
              {basename(r.path)}
            </option>
          ))}
        </select>
      </Field>

      <Field label="Modified after">
        <input
          type="date"
          className="w-full rounded-[6px] border border-hairline/10 bg-canvas px-2 py-1.5 text-body text-ink outline-none focus:border-accent"
          value={toISODate(filters.modified_from)}
          onChange={(e) =>
            setFilters({ modified_from: fromISO(e.target.value) })
          }
        />
      </Field>

      <Field label="Modified before">
        <input
          type="date"
          className="w-full rounded-[6px] border border-hairline/10 bg-canvas px-2 py-1.5 text-body text-ink outline-none focus:border-accent"
          value={toISODate(filters.modified_to)}
          onChange={(e) =>
            setFilters({ modified_to: fromISO(e.target.value) })
          }
        />
      </Field>

      <div className="mt-1 flex justify-between">
        <button
          className="rounded-[6px] px-2 py-1 text-caption text-subtle hover:bg-ink/5"
          onClick={() => useApp.getState().clearFilters()}
        >
          Clear all
        </button>
        <button
          className="rounded-[6px] bg-accent px-3 py-1 text-caption font-medium text-white"
          onClick={onClose}
        >
          Done
        </button>
      </div>
    </div>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="mb-2.5 block">
      <span className="mb-1 block text-caption font-medium text-subtle">
        {label}
      </span>
      {children}
    </label>
  );
}
