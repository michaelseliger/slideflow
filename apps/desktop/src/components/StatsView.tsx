import { useEffect, useState } from "react";
import {
  AlertTriangle,
  HardDrive,
  Loader2,
  RefreshCw,
} from "lucide-react";
import type { EmbeddingStatus, StatsOverview } from "../lib/types";
import { useApp } from "../stores/useApp";
import { useSemantic } from "../stores/useSemantic";
import { cx, deckDisplayName, formatBytes, formatModified, basename } from "../lib/utils";
import * as api from "../lib/api";
import { dropKindLabel } from "./ApproxBadge";

/** Statistics view: library counts, index timing, and recent activity
 *  (searches, exports). Swaps in for the grid via the sidebar. */
export default function StatsView() {
  const [overview, setOverview] = useState<StatsOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const scanRunning = useApp((s) => s.scan.running);
  const roots = useApp((s) => s.roots);
  const semanticStatus = useSemantic((s) => s.status);
  const aiIndexing = useSemantic((s) => s.indexing);

  const load = () => {
    api
      .getStatsOverview()
      .then((o) => {
        setOverview(o);
        setError(null);
      })
      .catch((err) => setError(String(err)));
  };

  // Refresh on entry and whenever an index run finishes.
  useEffect(() => {
    if (!scanRunning) load();
  }, [scanRunning]);

  if (error) {
    return (
      <div className="flex h-full items-center justify-center text-caption text-subtle">
        Couldn't load statistics: {error}
      </div>
    );
  }
  if (!overview) {
    return (
      <div className="flex h-full items-center justify-center text-caption text-subtle">
        Loading statistics…
      </div>
    );
  }

  const scan = overview.last_scan;

  return (
    <div className="h-full overflow-y-auto bg-canvas p-4">
      <div className="mx-auto max-w-[900px]">
        <div className="mb-4 flex items-center justify-end">
          <button
            onClick={load}
            className="flex items-center gap-1.5 rounded-[6px] px-2 py-1 text-caption text-subtle hover:bg-ink/5 hover:text-ink"
            title="Refresh"
          >
            <RefreshCw size={12} />
            Refresh
          </button>
        </div>

        {/* Stat tiles — value-first, iconless. */}
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <StatTile label="Slides indexed" value={compact(overview.slide_count)} />
          <StatTile label="Decks" value={compact(overview.deck_count)} />
          <StatTile label="Folders" value={compact(roots.length)} />
          <StatTile
            label="Favorites"
            value={compact(overview.favorite_slides)}
            detail={
              overview.favorite_decks > 0
                ? `+ ${overview.favorite_decks} deck${overview.favorite_decks === 1 ? "" : "s"}`
                : undefined
            }
          />
        </div>

        {/* Primary activity cards — 2×2. */}
        <div className="mt-4 grid grid-cols-1 gap-3.5 md:grid-cols-2">
          <Card title="Last index run">
            {scan ? (
              <>
                <div className="text-title font-semibold text-ink">
                  {formatModified(scan.started_unix)}
                </div>
                <div className="mt-0.5 text-caption text-subtle">
                  {formatDuration(scan.duration_ms)} · {scan.indexed} indexed ·{" "}
                  {scan.unchanged} unchanged · {scan.removed} removed
                </div>
              </>
            ) : (
              <div className="text-caption text-subtle">No index run recorded yet.</div>
            )}
          </Card>

          <Card title="AI index">
            <AiIndexBody status={semanticStatus} indexing={aiIndexing} />
          </Card>

          <Card title="Recent searches">
            {overview.recent_searches.length === 0 ? (
              <div className="text-caption text-subtle">
                Searches show up here once you start looking for slides.
              </div>
            ) : (
              <div className="flex flex-wrap gap-1.5">
                {overview.recent_searches.map((s, i) => (
                  <button
                    key={`${s.searched_unix}-${i}`}
                    title={`${s.result_count} hit${s.result_count === 1 ? "" : "s"} · run again`}
                    onClick={() => {
                      const app = useApp.getState();
                      void app.setNav({ type: "all" });
                      app.setQuery(s.query);
                    }}
                    className="max-w-full truncate rounded-full bg-ink/[0.06] px-2.5 py-1 text-caption text-ink transition-colors hover:bg-ink/10"
                  >
                    {s.query}
                  </button>
                ))}
              </div>
            )}
          </Card>

          <Card title="Recent exports">
            {overview.recent_exports.length === 0 ? (
              <div className="text-caption text-subtle">
                Exported decks show up here after your first composition.
              </div>
            ) : (
              <div className="flex flex-col gap-2">
                {overview.recent_exports.map((ex, i) => (
                  <button
                    key={`${ex.exported_unix}-${i}`}
                    title={ex.output_path}
                    onClick={() => void api.revealInFinder(ex.output_path)}
                    className="flex items-center gap-2 text-left"
                  >
                    <span className="min-w-0 flex-1 truncate text-body text-ink">
                      {ex.title}
                    </span>
                    <span className="tabnum shrink-0 text-caption text-subtle">
                      {ex.slide_count} slides
                    </span>
                    <span className="shrink-0 text-caption text-subtle/80">
                      {formatModified(ex.exported_unix)}
                    </span>
                  </button>
                ))}
              </div>
            )}
          </Card>
        </div>

        {/* Problems (hidden when the last run had no skips) */}
        {overview.last_scan_issues.length > 0 && (
          <Section icon={<AlertTriangle size={14} />} title="Problems">
            <ul>
              {overview.last_scan_issues.map((issue, i) => (
                <li
                  key={`${issue.path}-${i}`}
                  className="flex items-center gap-3 rounded-[6px] px-2 py-1.5 hover:bg-ink/5"
                  title={issue.path}
                >
                  <span className="min-w-0 flex-1 truncate text-body text-ink">
                    {basename(issue.path)}
                  </span>
                  <span
                    className="min-w-0 max-w-[55%] shrink-0 truncate text-caption text-subtle"
                    title={issue.reason}
                  >
                    {issue.reason}
                  </span>
                </li>
              ))}
            </ul>
          </Section>
        )}

        {/* Largest decks */}
        <Section icon={<HardDrive size={14} />} title="Largest decks">
          {overview.largest_decks.length === 0 ? (
            <Empty>Add a folder to start indexing decks.</Empty>
          ) : (
            <ul>
              {overview.largest_decks.map((d) => (
                <li key={d.id}>
                  <button
                    className="flex w-full items-center gap-3 rounded-[6px] px-2 py-1.5 text-left hover:bg-ink/5"
                    title={d.path}
                    onClick={() => void useApp.getState().setNav({ type: "deck", id: d.id })}
                  >
                    <span className="min-w-0 flex-1 truncate text-body text-ink">
                      {deckDisplayName(d)}
                    </span>
                    <span className="tabnum shrink-0 text-caption text-subtle">
                      {d.slide_count} slides
                    </span>
                    <span className="tabnum shrink-0 text-caption text-subtle/70">
                      {formatBytes(d.size_bytes)}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </Section>

        {/* Approximate previews: constructs the renderer skips (charts, etc.) */}
        <Section icon={<AlertTriangle size={14} />} title="Approximate previews">
          {overview.render_drops.length === 0 ? (
            <Empty>No unsupported content found in the previews you've opened yet.</Empty>
          ) : (
            <ul>
              {overview.render_drops.map((r) => (
                <li
                  key={r.kind}
                  className="flex items-center gap-3 rounded-[6px] px-2 py-1.5"
                >
                  <span className="min-w-0 flex-1 truncate text-body text-ink capitalize">
                    {dropKindLabel(r.kind)}
                  </span>
                  <span className="tabnum shrink-0 text-caption text-subtle">
                    {r.slides} slide{r.slides === 1 ? "" : "s"}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </Section>
      </div>
    </div>
  );
}

/** Compact value formatting for stat tiles (1,284 / 12.9K). */
function compact(n: number): string {
  if (n < 10_000) return n.toLocaleString();
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}K`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.floor(ms / 60_000)} min ${Math.round((ms % 60_000) / 1000)} s`;
}

function StatTile({
  label,
  value,
  detail,
}: {
  label: string;
  value: string;
  detail?: string;
}) {
  return (
    <div className="rounded-[8px] bg-surface px-4 py-3.5 shadow-tile">
      <div className="tabnum text-[26px] font-semibold leading-[30px] text-ink">
        {value}
      </div>
      <div className="mt-0.5 text-caption text-subtle">{label}</div>
      {detail && <div className="text-caption text-subtle/70">{detail}</div>}
    </div>
  );
}

/** A titled surface card for the stats activity grid. */
function Card({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="rounded-[8px] bg-surface px-4 py-3.5 shadow-tile">
      <div className="mb-2 text-caption font-semibold uppercase tracking-wide text-subtle/70">
        {title}
      </div>
      {children}
    </div>
  );
}

function Section({
  icon,
  title,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="mt-5">
      <h2 className="mb-1.5 flex items-center gap-1.5 text-caption font-semibold uppercase tracking-wide text-subtle/70">
        {icon}
        {title}
      </h2>
      <div className="rounded-[8px] bg-surface p-1.5 shadow-tile">{children}</div>
    </section>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <div className="px-2 py-3 text-caption text-subtle">{children}</div>;
}

const SEMANTIC_STATE: Record<
  EmbeddingStatus["state"],
  { label: string; dot: string }
> = {
  ready: { label: "Ready", dot: "bg-green-500" },
  disabled: { label: "Disabled", dot: "bg-ink/30" },
  not_downloaded: { label: "Not downloaded", dot: "bg-ink/30" },
  downloading: { label: "Downloading", dot: "bg-accent" },
  error: { label: "Error", dot: "bg-red-500" },
};

/** Body of the "AI index" section: feature state, model, coverage, and the live
 *  backfill bar — all read from the semantic store, no backend call. */
function AiIndexBody({
  status,
  indexing,
}: {
  status: EmbeddingStatus | null;
  indexing: { done: number; total: number } | null;
}) {
  if (!status) {
    return <Empty>Semantic-search status is loading…</Empty>;
  }
  const meta = SEMANTIC_STATE[status.state];
  const pending = Math.max(0, status.total_slides - status.embedded_slides);
  // Model + coverage are only meaningful once a model exists on disk.
  const hasModel = status.state === "ready" || status.embedded_slides > 0;

  return (
    <div className="space-y-1.5">
      <div className="flex items-center gap-2 text-body text-ink">
        <span className={cx("h-2 w-2 shrink-0 rounded-full", meta.dot)} />
        <span className="font-medium">{meta.label}</span>
        {status.state === "error" && status.error && (
          <span className="min-w-0 truncate text-caption text-subtle" title={status.error}>
            {status.error}
          </span>
        )}
      </div>

      {hasModel && (
        <>
          <div className="text-caption text-subtle/70">
            {status.model_id} · {status.dims}-dim
          </div>
          <div className="text-body text-ink">
            <span className="tabnum">{status.embedded_slides.toLocaleString()}</span> of{" "}
            <span className="tabnum">{status.total_slides.toLocaleString()}</span> slides indexed
            {pending > 0 && (
              <span className="text-subtle">
                {" · "}
                <span className="tabnum">{pending.toLocaleString()}</span> pending
              </span>
            )}
          </div>
        </>
      )}

      {indexing && (
        <div>
          <div className="flex items-center gap-1.5 text-caption text-subtle">
            <Loader2 size={12} className="animate-spin" />
            <span>
              AI indexing… <span className="tabnum">{indexing.done}</span> of{" "}
              <span className="tabnum">{indexing.total}</span>
            </span>
          </div>
          <div className="mt-1.5 h-1 overflow-hidden rounded-full bg-ink/10">
            <div
              className="h-full rounded-full bg-accent transition-[width] duration-300"
              style={{
                width: indexing.total
                  ? `${Math.min(100, (indexing.done / indexing.total) * 100)}%`
                  : "35%",
              }}
            />
          </div>
        </div>
      )}

      {(status.state === "disabled" || status.state === "not_downloaded") && (
        <div className="text-caption text-subtle">
          Enable semantic search in Settings to search your slides by meaning.
        </div>
      )}
    </div>
  );
}
