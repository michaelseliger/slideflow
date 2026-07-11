import { useEffect, useRef, useState } from "react";
import { FolderOpen, Plus, FileText, Check, Star, Sparkles, X } from "lucide-react";
import { useApp } from "../stores/useApp";
import { useTray, uidFor } from "../stores/useTray";
import { useSemantic } from "../stores/useSemantic";
import { toast } from "../stores/useToast";
import { deckDisplayName, formatModified, formatBytes, prefersReducedMotion } from "../lib/utils";
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
    hit ? s.items.some((i) => i.uid === uidFor(hit.deck, hit.slide)) : false,
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

          <h2 className="mt-3.5 text-heading font-semibold leading-tight text-ink">
            {hit.slide.title || deckDisplayName(hit.deck)}
          </h2>
          <div className="mt-1 text-body text-subtle">
            {deckDisplayName(hit.deck)} · Slide {hit.slide.slide_index} of{" "}
            {hit.deck.slide_count}
          </div>

          {hit.snippet && (
            <div
              className="selectable mt-2 rounded-[6px] bg-ink/5 p-2 text-caption leading-relaxed text-subtle [&_mark]:font-semibold"
              dangerouslySetInnerHTML={{ __html: hit.snippet }}
            />
          )}

          <dl className="mt-3.5 space-y-2 text-body">
            <Meta label="Deck" value={deckDisplayName(hit.deck)} />
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
          </dl>

          <TagEditor key={hit.slide.id} slideId={hit.slide.id} />

          {hit.slide.notes && (
            <div className="mt-4">
              <div className="mb-1.5 text-caption font-semibold uppercase tracking-wide text-subtle/70">
                Speaker notes
              </div>
              <div className="selectable whitespace-pre-wrap rounded-[6px] bg-ink/[0.04] px-2.5 py-2 text-body leading-relaxed text-ink">
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
              className="flex h-7 items-center justify-center gap-1.5 rounded-[6px] bg-accent text-body font-medium text-white transition-opacity hover:opacity-90 disabled:opacity-50"
            >
              {inTray ? <Check size={13} /> : <Plus size={13} />}
              {inTray ? "In tray" : "Add to Tray"}
            </button>
            <div className="flex gap-2">
              <button
                onClick={() => void useApp.getState().toggleFavoriteSlide(hit.slide.id)}
                title={hit.slide.favorite ? "Remove from Favorites" : "Add to Favorites"}
                className="flex h-7 flex-1 items-center justify-center gap-1.5 rounded-[6px] border border-hairline/10 text-body text-ink transition-colors hover:bg-ink/5"
              >
                <Star
                  size={13}
                  className={hit.slide.favorite ? "fill-current text-amber-400" : ""}
                />
                Favorite
              </button>
              <button
                onClick={() => void api.revealInFinder(hit.deck.path)}
                className="flex h-7 flex-1 items-center justify-center gap-1.5 rounded-[6px] border border-hairline/10 text-body text-ink transition-colors hover:bg-ink/5"
              >
                <FolderOpen size={13} />
                Reveal
              </button>
            </div>
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
  // `null` until the slide's existing tags have loaded. Persisting is a
  // full-set replace on the backend, so we must NOT mutate from an empty [] we
  // haven't confirmed — that would wipe the slide's real tags. The input stays
  // disabled and add/remove no-op until the fetch resolves (or rejects → []).
  const [names, setNames] = useState<string[] | null>(null);
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState(false);
  const listId = `taglist-${slideId}`;
  // Set once a local persist has happened; a late-resolving fetch must not
  // clobber the just-written set with stale DB state.
  const dirtyRef = useRef(false);

  useEffect(() => {
    let alive = true;
    dirtyRef.current = false;
    setNames(null);
    api
      .getSlideTags(slideId)
      .then((tags) => {
        if (alive && !dirtyRef.current) setNames(tags.map((t) => t.name));
      })
      .catch(() => {
        // Don't strand the input disabled forever; treat a failed load as
        // "no tags known yet" so the user can still add.
        if (alive && !dirtyRef.current) setNames([]);
      });
    return () => {
      alive = false;
    };
  }, [slideId]);

  const persist = (next: string[]) => {
    dirtyRef.current = true;
    setNames(next);
    void useApp.getState().setSlideTags(slideId, next);
  };

  const addDraft = () => {
    const name = draft.trim();
    setDraft("");
    if (!name || names == null) return;
    if (!names.some((n) => n.toLowerCase() === name.toLowerCase())) {
      persist([...names, name]);
    }
    // Stay in edit mode so several tags can be typed in a row.
    setEditing(true);
  };

  const remove = (name: string) => {
    if (names == null) return;
    persist(names.filter((n) => n !== name));
  };

  // Suggest existing tags not already applied to this slide.
  const applied = names ?? [];
  const suggestions = allTags
    .map((t) => t.name)
    .filter((n) => !applied.some((a) => a.toLowerCase() === n.toLowerCase()));

  return (
    <div className="mt-4">
      <div className="mb-2 text-caption font-semibold uppercase tracking-wide text-subtle/70">
        Tags
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        {names?.map((n) => (
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
        {editing ? (
          <input
            autoFocus
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                addDraft();
              } else if (e.key === "Escape") {
                e.preventDefault();
                setDraft("");
                setEditing(false);
              } else if (
                e.key === "Backspace" &&
                draft === "" &&
                names &&
                names.length > 0
              ) {
                remove(names[names.length - 1]);
              }
            }}
            onBlur={() => {
              addDraft();
              setEditing(false);
            }}
            list={listId}
            disabled={names == null}
            placeholder="Add a tag…"
            className="h-[22px] w-28 rounded-full border border-accent bg-canvas px-2.5 text-caption text-ink outline-hidden disabled:opacity-50"
          />
        ) : (
          <button
            onClick={() => setEditing(true)}
            disabled={names == null}
            className="flex items-center gap-1 rounded-full border border-dashed border-hairline/20 px-2 py-0.5 text-caption text-subtle transition-colors hover:border-accent hover:text-accent disabled:opacity-50"
          >
            <Plus size={12} />
            {names == null ? "Loading…" : "Add tag"}
          </button>
        )}
      </div>
      <datalist id={listId}>
        {suggestions.map((n) => (
          <option key={n} value={n} />
        ))}
      </datalist>
    </div>
  );
}

// The last find-similar nonce we scrolled for, kept at MODULE scope (not a ref)
// so a nonce is consumed exactly once across the whole app. similarRequestNonce
// persists in the store, but this component unmounts/remounts constantly
// (deselect, inspector toggle, refresh) — a per-instance ref would reset to 0 and
// re-fire scrollIntoView on a plain selection.
let lastConsumedSimilarNonce = 0;

/** "Similar slides" (AI): the top semantic neighbors of the inspected slide.
 *  Rendered only while the model is ready; loads lazily per selection. An
 *  explicit "Find similar" bumps `similarRequestNonce`, which re-runs the fetch
 *  (even when the slide is unchanged) and scrolls this section into view. */
function SimilarSlides({ slideId }: { slideId: number }) {
  const ready = useSemantic((s) => s.status?.state === "ready");
  const nonce = useApp((s) => s.similarRequestNonce);
  const [similar, setSimilar] = useState<SimilarSlide[] | null>(null);
  const sectionRef = useRef<HTMLDivElement>(null);

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
  }, [slideId, ready, nonce]);

  // Scroll into view only for an EXPLICIT find-similar request (nonce ≥ 1),
  // never on a normal inspector open (nonce 0), so we don't yank the scroll
  // position. Each nonce triggers at most one scroll, globally — the marker is
  // module-scoped so a remount on a plain selection can't re-consume it.
  useEffect(() => {
    if (!ready || nonce === 0 || lastConsumedSimilarNonce === nonce) return;
    lastConsumedSimilarNonce = nonce;
    sectionRef.current?.scrollIntoView({
      behavior: prefersReducedMotion() ? "auto" : "smooth",
      block: "nearest",
    });
  }, [nonce, ready]);

  if (!ready) return null;

  return (
    <div ref={sectionRef} className="mt-4">
      <div className="mb-2 flex items-center gap-1 text-caption font-semibold uppercase tracking-wide text-subtle/70">
        <Sparkles size={13} />
        Similar slides
      </div>
      {similar == null ? (
        <div className="text-caption text-subtle">Searching…</div>
      ) : similar.length === 0 ? (
        <div className="text-caption text-subtle">No similar slides found.</div>
      ) : (
        <div className="space-y-2.5">
          {similar.map((s) => (
            <button
              key={s.slide.id}
              onClick={() => void useApp.getState().setNav({ type: "deck", id: s.deck.id })}
              title={`Open ${deckDisplayName(s.deck)}`}
              className="flex w-full items-center gap-2.5 rounded-[6px] text-left transition-colors hover:bg-ink/5"
            >
              <div className="w-14 shrink-0 overflow-hidden rounded-[4px] shadow-tile">
                <Thumbnail slideId={s.slide.id} rounded={false} />
              </div>
              <div className="min-w-0 flex-1">
                <div className="truncate text-body font-medium text-ink">
                  {s.slide.title || deckDisplayName(s.deck)}
                </div>
                <div className="truncate text-caption text-subtle">
                  {deckDisplayName(s.deck)}
                </div>
              </div>
              <span
                className="tabnum shrink-0 text-caption font-semibold"
                style={{ color: "var(--near-duplicate)" }}
              >
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
      <dt className="w-[70px] shrink-0 text-subtle">{label}</dt>
      <dd
        className={`selectable min-w-0 flex-1 truncate text-ink ${
          mono ? "font-mono text-[11px]" : ""
        }`}
        title={value}
      >
        {value}
      </dd>
    </div>
  );
}
