import { useRef, useState } from "react";
import { useApp } from "../stores/useApp";
import { useDismiss } from "../lib/useDismiss";

/** Names and persists the current query as a saved search. */
export default function SaveSearchPopover({ onClose }: { onClose: () => void }) {
  const query = useApp((s) => s.query);
  const saveCurrentSearch = useApp((s) => s.saveCurrentSearch);
  const [name, setName] = useState(() => suggestName(query));
  const ref = useRef<HTMLDivElement>(null);

  useDismiss(ref, onClose);

  const submit = () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    void saveCurrentSearch(trimmed);
    onClose();
  };

  return (
    <div
      ref={ref}
      className="absolute right-0 top-full z-50 mt-1.5 w-72 rounded-[10px] border border-hairline/10 bg-elevated p-4 text-body shadow-peek"
      onClick={(e) => e.stopPropagation()}
    >
      <div className="text-body font-semibold text-ink">Save this search</div>
      {query.trim() && (
        <div className="mt-0.5 truncate text-caption text-subtle" title={query.trim()}>
          {query.trim()}
        </div>
      )}
      <input
        autoFocus
        spellCheck={false}
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") submit();
        }}
        placeholder="Name"
        className="selectable mt-3 w-full rounded-[7px] border border-accent bg-canvas px-2.5 py-1.5 text-body text-ink outline-hidden focus:border-accent"
      />
      <div className="mt-2.5 flex justify-end gap-1.5">
        <button
          className="rounded-[6px] px-2 py-1 text-caption text-subtle hover:bg-ink/5"
          onClick={onClose}
        >
          Cancel
        </button>
        <button
          className="rounded-[6px] bg-accent px-3 py-1 text-caption font-medium text-white disabled:opacity-40"
          onClick={submit}
          disabled={!name.trim()}
        >
          Save
        </button>
      </div>
    </div>
  );
}

/** Seed the name field from the query, trimmed to a sensible length. */
function suggestName(query: string): string {
  const q = query.trim();
  if (!q) return "";
  return q.length > 40 ? q.slice(0, 40) : q;
}
