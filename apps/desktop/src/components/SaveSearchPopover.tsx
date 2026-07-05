import { useEffect, useRef, useState } from "react";
import { useApp } from "../stores/useApp";

/** Names and persists the current query + active filters as a saved search. */
export default function SaveSearchPopover({ onClose }: { onClose: () => void }) {
  const query = useApp((s) => s.query);
  const saveCurrentSearch = useApp((s) => s.saveCurrentSearch);
  const [name, setName] = useState(() => suggestName(query));
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

  const submit = () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    void saveCurrentSearch(trimmed);
    onClose();
  };

  return (
    <div
      ref={ref}
      className="absolute right-0 top-full z-50 mt-1.5 w-64 rounded-[8px] border border-hairline/10 bg-elevated p-3 text-body shadow-peek"
      onClick={(e) => e.stopPropagation()}
    >
      <div className="mb-1.5 text-caption font-medium text-subtle">Save this search</div>
      <input
        autoFocus
        spellCheck={false}
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") submit();
        }}
        placeholder="Name"
        className="selectable w-full rounded-[6px] border border-hairline/10 bg-canvas px-2 py-1.5 text-body text-ink outline-none focus:border-accent"
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
