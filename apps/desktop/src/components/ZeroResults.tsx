import { Search, RefreshCw } from "lucide-react";
import { useApp } from "../stores/useApp";
import { basename } from "../lib/utils";

/** Calm zero-results state: confirms scope, offers re-index, suggests dropping
 *  the most restrictive filter, and never dead-ends. */
export default function ZeroResults() {
  const roots = useApp((s) => s.roots);
  const filters = useApp((s) => s.filters);
  const query = useApp((s) => s.query);

  const fmtMonth = (unix: number) => {
    const d = new Date(unix * 1000);
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
  };

  // Identify the "most restrictive" active filter to offer removing, labelled as
  // the search token it corresponds to.
  const restrictive: { label: string; clear: () => void } | null = filters.deck_query
    ? {
        label: `deck:"${filters.deck_query}"`,
        clear: () => useApp.getState().setFilters({ deck_query: null }),
      }
    : filters.path_prefix
      ? {
          label: `folder:${basename(filters.path_prefix)}`,
          clear: () => useApp.getState().setFilters({ path_prefix: null }),
        }
      : filters.modified_from
        ? {
            label: `after:${fmtMonth(filters.modified_from)}`,
            clear: () => useApp.getState().setFilters({ modified_from: null }),
          }
        : filters.modified_to
          ? {
              label: `before:${fmtMonth(filters.modified_to)}`,
              clear: () => useApp.getState().setFilters({ modified_to: null }),
            }
          : null;

  return (
    <div className="flex h-full flex-col items-center justify-center bg-canvas px-8 text-center text-subtle">
      <Search size={36} className="opacity-50" />
      <h2 className="mt-3.5 text-heading font-semibold leading-snug text-ink">
        {query.trim() ? `No slides match “${query.trim()}”` : "Nothing here yet"}
      </h2>

      <p className="mt-1.5 max-w-[340px] text-body text-subtle">
        {roots.length > 0 ? (
          <>
            Searched <span className="text-ink">{scopeName(roots)}</span>. Try
            removing your most restrictive filter, or re-index in case the files
            moved.
          </>
        ) : (
          "Add a folder to start building your slide library."
        )}
      </p>

      <div className="mt-5 flex flex-wrap items-center justify-center gap-2">
        {restrictive && (
          <button
            onClick={restrictive.clear}
            className="rounded-[6px] border border-hairline/10 px-3 py-1.5 text-caption text-ink hover:bg-ink/5"
          >
            Remove “{restrictive.label}”
          </button>
        )}
        <button
          onClick={() => void useApp.getState().startScan()}
          className="flex items-center gap-1.5 rounded-[6px] border border-hairline/10 px-3 py-1.5 text-caption text-ink hover:bg-ink/5"
        >
          <RefreshCw size={13} />
          Re-index folders
        </button>
      </div>
    </div>
  );
}

/** The searched scope: the single folder's name, or "All Slides" across many. */
function scopeName(roots: { path: string }[]): string {
  if (roots.length === 1) return basename(roots[0].path);
  return "All Slides";
}
