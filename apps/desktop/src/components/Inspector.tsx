import { useEffect, useState } from "react";
import { FolderOpen, Plus, FileText, Check, Star, Sparkles, X } from "lucide-react";
import { useApp } from "../stores/useApp";
import { useTray } from "../stores/useTray";
import { useSemantic } from "../stores/useSemantic";
import { toast } from "../stores/useToast";
import { deckDisplayName, formatModified, formatBytes } from "../lib/utils";
import * as api from "../lib/api";
import type { SimilarSlide } from "../lib/types";
import Thumbnail from "./Thumbnail";

/** Right inspector: large preview + source metadata + matched-text context for
 *  the single selected slide. Opaque background so slide colors read true. */
export default function Inspector() {
  const results = useApp((s) => s.results);
  const selectedIds = useApp((s) => s.selectedIds);

  // Inspect the first selected slide (in visible order).
  const hit = results.find((r) => selectedIds.has(r.slide.id));
  const inTray = useTray((s) =>
    hit ? s.items.some((i) => i.uid === `${hit.slide.deck_id}:${hit.slide.slide_index}`) : false,
  );

  return (
    <aside className="hairline-l flex w-[300px] shrink-0 flex-col bg-surface">
      <div className="h-[52px] shrink-0" data-tauri-drag-region />
      {!hit ? (
        <div className="flex flex-1 flex-col items-center justify-center px-6 text-center text-caption text-subtle">
          <FileText size={28} className="mb-2 opacity-40" />
          Select a slide to inspect its source, metadata, and matched text.
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto p-4">
          <div className="overflow-hidden rounded-[8px] shadow-tile">
            <Thumbnail slideId={hit.slide.id} rounded={false} />
          </div>

          <h2 className="mt-3 text-title font-semibold text-ink">
            {hit.slide.title || deckDisplayName(hit.deck)}
          </h2>

          {hit.snippet && (
            <div
              className="selectable mt-1.5 rounded-[6px] bg-ink/5 p-2 text-caption leading-relaxed text-subtle [&_mark]:font-semibold"
              dangerouslySetInnerHTML={{ __html: hit.snippet }}
            />
          )}

          <dl className="mt-3 space-y-2 text-caption">
            <Meta label="Source deck" value={deckDisplayName(hit.deck)} />
            <Meta label="File" value={hit.deck.file_name} mono />
            {hit.deck.title &&
              hit.deck.title !== deckDisplayName(hit.deck) && (
                <Meta label="Doc title" value={hit.deck.title} />
              )}
            <Meta
              label="Slide"
              value={`${hit.slide.slide_index} of ${hit.deck.slide_count}`}
            />
            <Meta label="Modified" value={formatModified(hit.deck.modified_unix)} />
            <Meta label="Size" value={formatBytes(hit.deck.size_bytes)} />
            {hit.deck.author && <Meta label="Author" value={hit.deck.author} />}
            <Meta label="Path" value={hit.deck.path} mono />
          </dl>

          <TagEditor key={hit.slide.id} slideId={hit.slide.id} />

          {hit.slide.notes && (
            <div className="mt-3">
              <div className="mb-1 text-caption font-semibold uppercase tracking-wide text-subtle/70">
                Speaker notes
              </div>
              <div className="selectable whitespace-pre-wrap rounded-[6px] bg-ink/5 p-2 text-caption leading-relaxed text-subtle">
                {hit.slide.notes}
              </div>
            </div>
          )}

          <SimilarSlides slideId={hit.slide.id} />

          <div className="mt-4 flex flex-col gap-2">
            <button
              onClick={() => {
                const added = useTray
                  .getState()
                  .add([{ slide: hit.slide, deck: hit.deck }]);
                if (added > 0) toast.success("Added to the tray");
              }}
              disabled={inTray}
              className="flex items-center justify-center gap-2 rounded-[6px] bg-accent py-2 text-body font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
            >
              {inTray ? <Check size={15} /> : <Plus size={15} />}
              {inTray ? "In tray" : "Add to Tray"}
            </button>
            <button
              onClick={() => void useApp.getState().toggleFavoriteSlide(hit.slide.id)}
              className="flex items-center justify-center gap-2 rounded-[6px] border border-hairline/10 py-2 text-body text-ink transition-colors hover:bg-ink/5"
            >
              <Star
                size={15}
                className={hit.slide.favorite ? "fill-current text-amber-400" : ""}
              />
              {hit.slide.favorite ? "Remove from Favorites" : "Add to Favorites"}
            </button>
            <button
              onClick={() => void api.revealInFinder(hit.deck.path)}
              className="flex items-center justify-center gap-2 rounded-[6px] border border-hairline/10 py-2 text-body text-ink transition-colors hover:bg-ink/5"
            >
              <FolderOpen size={15} />
              Reveal in Finder
            </button>
          </div>
        </div>
      )}
    </aside>
  );
}

/** Tags editor for the selected slide: removable chips + an input that
 *  autocompletes existing tags (via a datalist) and creates a tag on Enter.
 *  Persists the full set through the store, which reloads the sidebar counts. */
function TagEditor({ slideId }: { slideId: number }) {
  const allTags = useApp((s) => s.tags);
  const [names, setNames] = useState<string[]>([]);
  const [draft, setDraft] = useState("");
  const listId = `taglist-${slideId}`;

  useEffect(() => {
    let alive = true;
    void api.getSlideTags(slideId).then((tags) => {
      if (alive) setNames(tags.map((t) => t.name));
    });
    return () => {
      alive = false;
    };
  }, [slideId]);

  const persist = (next: string[]) => {
    setNames(next);
    void useApp.getState().setSlideTags(slideId, next);
  };

  const addDraft = () => {
    const name = draft.trim();
    setDraft("");
    if (!name) return;
    if (!names.some((n) => n.toLowerCase() === name.toLowerCase())) {
      persist([...names, name]);
    }
  };

  const remove = (name: string) => persist(names.filter((n) => n !== name));

  // Suggest existing tags not already applied to this slide.
  const suggestions = allTags
    .map((t) => t.name)
    .filter((n) => !names.some((applied) => applied.toLowerCase() === n.toLowerCase()));

  return (
    <div className="mt-3">
      <div className="mb-1 text-caption font-semibold uppercase tracking-wide text-subtle/70">
        Tags
      </div>
      {names.length > 0 && (
        <div className="mb-1.5 flex flex-wrap gap-1">
          {names.map((n) => (
            <span
              key={n}
              className="flex items-center gap-1 rounded-full bg-ink/8 py-0.5 pl-2 pr-1 text-caption text-ink"
            >
              {n}
              <button
                onClick={() => remove(n)}
                title={`Remove ${n}`}
                className="flex h-4 w-4 items-center justify-center rounded-full text-subtle hover:bg-ink/10 hover:text-ink"
              >
                <X size={11} />
              </button>
            </span>
          ))}
        </div>
      )}
      <input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            addDraft();
          } else if (e.key === "Backspace" && draft === "" && names.length > 0) {
            remove(names[names.length - 1]);
          }
        }}
        list={listId}
        placeholder="Add a tag…"
        className="h-7 w-full rounded-[5px] border border-hairline/10 bg-canvas px-2 text-body text-ink outline-none focus:border-accent"
      />
      <datalist id={listId}>
        {suggestions.map((n) => (
          <option key={n} value={n} />
        ))}
      </datalist>
    </div>
  );
}

/** "Similar slides" (AI): the top semantic neighbors of the inspected slide.
 *  Rendered only while the model is ready; loads lazily per selection. */
function SimilarSlides({ slideId }: { slideId: number }) {
  const ready = useSemantic((s) => s.status?.state === "ready");
  const [similar, setSimilar] = useState<SimilarSlide[] | null>(null);

  useEffect(() => {
    if (!ready) return;
    let alive = true;
    setSimilar(null);
    api
      .getSimilarSlides(slideId, 6)
      .then((res) => {
        if (alive) setSimilar(res);
      })
      .catch(() => {
        if (alive) setSimilar([]);
      });
    return () => {
      alive = false;
    };
  }, [slideId, ready]);

  if (!ready) return null;

  return (
    <div className="mt-3">
      <div className="mb-1 flex items-center gap-1 text-caption font-semibold uppercase tracking-wide text-subtle/70">
        <Sparkles size={11} />
        Similar slides
      </div>
      {similar == null ? (
        <div className="text-caption text-subtle">Searching…</div>
      ) : similar.length === 0 ? (
        <div className="text-caption text-subtle">No similar slides found.</div>
      ) : (
        <div className="space-y-1.5">
          {similar.map((s) => (
            <button
              key={s.slide.id}
              onClick={() => void useApp.getState().setNav({ type: "deck", id: s.deck.id })}
              title={`Open ${deckDisplayName(s.deck)}`}
              className="flex w-full items-center gap-2 rounded-[6px] p-1 text-left transition-colors hover:bg-ink/5"
            >
              <div className="w-16 shrink-0 overflow-hidden rounded-[4px] shadow-tile">
                <Thumbnail slideId={s.slide.id} rounded={false} />
              </div>
              <div className="min-w-0 flex-1">
                <div className="truncate text-caption font-medium text-ink">
                  {s.slide.title || deckDisplayName(s.deck)}
                </div>
                <div className="truncate text-caption text-subtle">
                  {deckDisplayName(s.deck)} · #{s.slide.slide_index}
                </div>
              </div>
              <span className="tabnum shrink-0 text-caption text-subtle/80">
                {Math.round(s.score * 100)}%
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function Meta({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="flex gap-2">
      <dt className="w-20 shrink-0 text-subtle/70">{label}</dt>
      <dd
        className={`selectable min-w-0 flex-1 break-words text-ink ${
          mono ? "font-mono text-[11px]" : ""
        }`}
      >
        {value}
      </dd>
    </div>
  );
}
