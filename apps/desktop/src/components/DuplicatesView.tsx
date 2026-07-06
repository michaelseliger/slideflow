import { useCallback, useEffect, useState } from "react";
import { Copy, RefreshCw, Sparkles } from "lucide-react";
import * as api from "../lib/api";
import type { DuplicateGroup } from "../lib/types";
import { useApp } from "../stores/useApp";
import { useSemantic } from "../stores/useSemantic";
import { deckDisplayName, formatModified } from "../lib/utils";
import Thumbnail from "./Thumbnail";

/** Duplicates view (sidebar → Duplicates): clusters of identical (exact) and —
 *  with the semantic model — near-identical slides across the whole library.
 *  Self-fetching like StatsView; the slide grid stays empty behind it. */
export default function DuplicatesView() {
  const [groups, setGroups] = useState<DuplicateGroup[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const semanticReady = useSemantic((s) => s.status?.state === "ready");

  const load = useCallback(() => {
    setGroups(null);
    setError(null);
    api
      .listDuplicateGroups()
      .then(setGroups)
      // A failed command must not masquerade as the empty-success state
      // ("No duplicates found"); surface it so Refresh is an obvious retry.
      .catch((err) => setError(String(err)));
  }, []);

  // Reload when the model comes up (near groups appear) and on rescans.
  const scanRunning = useApp((s) => s.scan.running);
  useEffect(() => {
    if (!scanRunning) load();
  }, [load, semanticReady, scanRunning]);

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-5xl px-6 py-6">
        <div className="mb-4 flex items-center justify-between">
          <div>
            <h1 className="text-title font-semibold text-ink">Duplicates</h1>
            <p className="mt-0.5 text-caption text-subtle">
              Slides that appear more than once across your decks
              {semanticReady ? ", including near-identical copies." : "."}
            </p>
          </div>
          <button
            onClick={load}
            title="Refresh"
            className="flex h-8 items-center gap-1.5 rounded-[6px] border border-hairline/10 px-2.5 text-caption text-subtle hover:bg-ink/5 hover:text-ink"
          >
            <RefreshCw size={13} />
            Refresh
          </button>
        </div>

        {!semanticReady && (
          <p className="mb-4 rounded-[8px] bg-ink/5 px-3 py-2 text-caption text-subtle">
            Showing exact copies only. Enable semantic search in Settings to also find
            near-identical slides (reworded or restyled copies).
          </p>
        )}

        {error != null ? (
          <div className="flex flex-col items-center py-20 text-center">
            <Copy size={28} className="mb-3 text-subtle/40" />
            <div className="text-body font-medium text-ink">Couldn't scan for duplicates</div>
            <p className="mt-1 max-w-sm text-caption text-subtle">{error}</p>
          </div>
        ) : groups == null ? (
          <div className="py-16 text-center text-caption text-subtle">Scanning for duplicates…</div>
        ) : groups.length === 0 ? (
          <div className="flex flex-col items-center py-20 text-center">
            <Copy size={28} className="mb-3 text-subtle/40" />
            <div className="text-body font-medium text-ink">No duplicates found</div>
            <p className="mt-1 max-w-sm text-caption text-subtle">
              Every indexed slide is unique{semanticReady ? " — exact and near-identical" : ""}.
            </p>
          </div>
        ) : (
          <div className="space-y-6">
            {groups.map((g, i) => (
              <Group key={`${g.kind}-${i}`} group={g} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function Group({ group }: { group: DuplicateGroup }) {
  const near = group.kind === "near";
  return (
    <section>
      <div className="mb-2.5 flex items-center gap-2">
        <span className="flex shrink-0 items-center" style={near ? { color: "var(--near-duplicate)" } : undefined}>
          {near ? (
            <Sparkles size={16} />
          ) : (
            <Copy size={16} className="text-subtle" />
          )}
        </span>
        <span className="text-title font-semibold text-ink">
          {near ? "Near duplicate" : "Exact duplicate"}
        </span>
        <span className="text-caption text-subtle">
          {group.slides.length} copies
          {near ? "" : " · identical content hash"}
        </span>
        {near && group.score != null && (
          <span
            className="rounded-full px-2 py-0.5 text-caption font-semibold"
            style={{ background: "rgb(168 85 247 / 0.14)", color: "var(--near-duplicate)" }}
          >
            {Math.round(group.score * 100)}% similar
          </span>
        )}
      </div>

      <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
        {group.slides.map((s, idx) => (
          <button
            key={s.slide.id}
            onClick={() => void useApp.getState().setNav({ type: "deck", id: s.deck.id })}
            title={`Open ${deckDisplayName(s.deck)}`}
            className="group rounded-[8px] bg-surface p-1.5 text-left shadow-tile transition-shadow hover:shadow-tile-hover"
          >
            <div className="relative">
              <Thumbnail
                slideId={s.slide.id}
                alt={s.slide.title ?? deckDisplayName(s.deck)}
              />
              {idx === 0 && (
                <span className="absolute left-3 top-3 rounded-full bg-ink px-1.5 py-0.5 text-[10px] font-semibold text-surface shadow">
                  Newest
                </span>
              )}
            </div>
            <div className="px-0.5 pb-0.5 pt-1.5">
              <div className="truncate text-caption font-medium text-ink">
                {s.slide.title || deckDisplayName(s.deck)}
              </div>
              <div className="mt-0.5 truncate text-caption text-subtle" title={s.deck.path}>
                {deckDisplayName(s.deck)} · #{s.slide.slide_index}
              </div>
              <div className="tabnum mt-0.5 text-caption text-subtle/80">
                {formatModified(s.deck.modified_unix)}
              </div>
            </div>
          </button>
        ))}
      </div>
    </section>
  );
}
