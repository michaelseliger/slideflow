import { useRef, useState } from "react";
import { ArrowDownUp, Check } from "lucide-react";
import { useApp, type SortMode } from "../stores/useApp";
import { cx } from "../lib/utils";
import { useDismiss } from "../lib/useDismiss";

const OPTIONS: { mode: SortMode; label: string; hint?: string }[] = [
  { mode: "name", label: "Name" },
  { mode: "added", label: "Recently added" },
  { mode: "modified", label: "Recently modified" },
  { mode: "exported", label: "Most exported", hint: "Counting starts now" },
];

const SHORT: Record<SortMode, string> = {
  name: "Name",
  added: "Added",
  modified: "Modified",
  exported: "Exported",
};

/** Compact sort control for the browse grid. Sort applies only while browsing;
 *  during a search the trigger dims and the reorder defers until the query
 *  clears (search stays bm25-ranked). Self-contained open state. */
export default function SortMenu() {
  const sortMode = useApp((s) => s.sortMode);
  const setSortMode = useApp((s) => s.setSortMode);
  // Sort is inert during a search (bm25-ranked) or single-deck nav (fixed slide
  // order); dim the trigger in both cases. The reorder defers until browsing.
  const inert = useApp((s) => s.query.trim() !== "" || s.nav.type === "deck");
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useDismiss(ref, () => setOpen(false), { enabled: open });

  return (
    <div ref={ref} className="relative">
      <button
        title={
          inert
            ? "Sorting applies while browsing (search and deck views keep their own order)"
            : "Sort slides"
        }
        onClick={() => setOpen((v) => !v)}
        className={cx(
          "flex h-6 items-center gap-1 rounded-[6px] border border-hairline/10 px-1.5 text-caption transition-colors",
          inert && "opacity-50",
          open ? "bg-accent/10 text-accent" : "text-subtle hover:bg-ink/8",
        )}
      >
        <ArrowDownUp size={12} />
        <span>{SHORT[sortMode]}</span>
      </button>

      {open && (
        <div className="absolute right-0 top-full z-50 mt-1.5 w-52 rounded-[8px] border border-hairline/10 bg-elevated p-1 shadow-peek">
          <div className="px-2 py-1 text-caption font-medium text-subtle">Sort by</div>
          {OPTIONS.map((o) => (
            <button
              key={o.mode}
              onClick={() => {
                setSortMode(o.mode);
                setOpen(false);
              }}
              className="flex w-full items-center gap-2 rounded-[6px] px-2 py-1.5 text-body text-ink hover:bg-ink/8"
            >
              <Check
                size={13}
                className={o.mode === sortMode ? "text-accent opacity-100" : "opacity-0"}
              />
              <span className="flex-1 text-left">{o.label}</span>
              {o.hint && <span className="text-[10px] text-subtle/70">{o.hint}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
