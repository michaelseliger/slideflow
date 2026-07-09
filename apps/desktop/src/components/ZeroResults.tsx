import { Search, RefreshCw } from "lucide-react";
import { useApp } from "../stores/useApp";
import { basename } from "../lib/utils";

/** Calm zero-results state: confirms scope, offers re-index, and never
 *  dead-ends. */
export default function ZeroResults() {
  const roots = useApp((s) => s.roots);
  const query = useApp((s) => s.query);

  return (
    <div className="flex h-full flex-col items-center justify-center bg-canvas px-8 text-center text-subtle">
      <Search size={36} className="opacity-50" />
      <h2 className="mt-3.5 text-heading font-semibold leading-snug text-ink">
        {query.trim() ? `No slides match “${query.trim()}”` : "Nothing here yet"}
      </h2>

      <p className="mt-1.5 max-w-[340px] text-body text-subtle">
        {roots.length > 0 ? (
          <>
            Searched <span className="text-ink">{scopeName(roots)}</span>. Try a
            broader query, or re-index in case the files moved.
          </>
        ) : (
          "Add a folder to start building your slide library."
        )}
      </p>

      <div className="mt-5 flex flex-wrap items-center justify-center gap-2">
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
