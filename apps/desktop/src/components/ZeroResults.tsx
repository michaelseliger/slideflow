import { SearchX, RefreshCw } from "lucide-react";
import { useApp } from "../stores/useApp";
import { basename } from "../lib/utils";

/** Calm zero-results state: confirms scope, offers re-index, suggests dropping
 *  the most restrictive filter, and never dead-ends. */
export default function ZeroResults() {
  const roots = useApp((s) => s.roots);
  const filters = useApp((s) => s.filters);
  const query = useApp((s) => s.query);

  // Identify the "most restrictive" active filter to offer removing.
  const restrictive: { label: string; clear: () => void } | null = filters.deck_query
    ? {
        label: `Deck: ${filters.deck_query}`,
        clear: () => useApp.getState().setFilters({ deck_query: null }),
      }
    : filters.path_prefix
      ? {
          label: `Folder: ${basename(filters.path_prefix)}`,
          clear: () => useApp.getState().setFilters({ path_prefix: null }),
        }
      : filters.modified_from
        ? {
            label: "Modified-after date",
            clear: () => useApp.getState().setFilters({ modified_from: null }),
          }
        : filters.modified_to
          ? {
              label: "Modified-before date",
              clear: () => useApp.getState().setFilters({ modified_to: null }),
            }
          : null;

  return (
    <div className="flex h-full flex-col items-center justify-center bg-canvas px-8 text-center">
      <SearchX size={36} className="text-subtle/50" />
      <h2 className="mt-4 text-title font-semibold text-ink">
        {query.trim() ? `No slides match “${query.trim()}”` : "Nothing here yet"}
      </h2>

      <p className="mt-2 max-w-md text-caption text-subtle">
        {roots.length > 0 ? (
          <>
            Searched{" "}
            <span className="text-ink">
              {roots.map((r) => basename(r.path)).join(", ")}
            </span>
            .
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
            Remove filter · {restrictive.label}
          </button>
        )}
        <button
          onClick={() => void useApp.getState().startScan()}
          className="flex items-center gap-1.5 rounded-[6px] border border-hairline/10 px-3 py-1.5 text-caption text-ink hover:bg-ink/5"
        >
          <RefreshCw size={13} />
          Search failed? Re-index folders
        </button>
      </div>
    </div>
  );
}
