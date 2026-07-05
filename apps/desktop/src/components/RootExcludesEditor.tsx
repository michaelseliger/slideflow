import { useState } from "react";
import { useApp } from "../stores/useApp";
import type { RootRecord } from "../lib/types";

/** Per-root exclude-glob editor, mounted under each folder row in the Settings
 *  Library section. One glob per line; Save validates backend-side and kicks a
 *  rescan so newly-excluded decks drop out of the index on the next pass. */
export default function RootExcludesEditor({ root }: { root: RootRecord }) {
  const stored = root.exclude_globs.join("\n");
  const [text, setText] = useState(stored);
  const [saving, setSaving] = useState(false);
  const dirty = text !== stored;

  const save = async () => {
    const patterns = text
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
    setSaving(true);
    try {
      await useApp.getState().setRootExcludes(root.id, patterns);
      // Reflect the normalized stored value so `dirty` resets after a save.
      setText(patterns.join("\n"));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mb-1 mt-0.5 pl-7 pr-2">
      <div className="mb-1 text-caption font-medium text-subtle">
        Exclude patterns
      </div>
      <textarea
        rows={3}
        spellCheck={false}
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder={"**/archive/**\n~$*"}
        className="selectable w-full resize-y rounded-[6px] border border-hairline/10 bg-canvas px-2.5 py-2 font-mono text-caption text-ink outline-none placeholder:text-subtle/50 focus:border-accent"
      />
      <div className="mt-1 flex items-start justify-between gap-3">
        <p className="text-caption text-subtle">
          One glob per line, relative to this folder. Examples: **/archive/**, ~$*
        </p>
        <button
          onClick={() => void save()}
          disabled={!dirty || saving}
          className="shrink-0 rounded-[6px] bg-accent px-3 py-1 text-caption font-medium text-white hover:opacity-90 disabled:opacity-40"
        >
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  );
}
