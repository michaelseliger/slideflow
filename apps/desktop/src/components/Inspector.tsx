import { FolderOpen, Plus, FileText, Check } from "lucide-react";
import { useApp } from "../stores/useApp";
import { useTray } from "../stores/useTray";
import { toast } from "../stores/useToast";
import { formatModified, formatBytes } from "../lib/utils";
import * as api from "../lib/api";
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
            {hit.slide.title || hit.deck.title}
          </h2>

          {hit.snippet && (
            <div
              className="selectable mt-1.5 rounded-[6px] bg-ink/5 p-2 text-caption leading-relaxed text-subtle [&_mark]:font-semibold"
              dangerouslySetInnerHTML={{ __html: hit.snippet }}
            />
          )}

          <dl className="mt-3 space-y-2 text-caption">
            <Meta label="Source deck" value={hit.deck.title} />
            <Meta label="File" value={hit.deck.file_name} mono />
            <Meta
              label="Slide"
              value={`${hit.slide.slide_index} of ${hit.deck.slide_count}`}
            />
            <Meta label="Modified" value={formatModified(hit.deck.modified_unix)} />
            <Meta label="Size" value={formatBytes(hit.deck.size_bytes)} />
            {hit.deck.author && <Meta label="Author" value={hit.deck.author} />}
            <Meta label="Path" value={hit.deck.path} mono />
          </dl>

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
