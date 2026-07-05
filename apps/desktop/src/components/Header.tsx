import {
  Search,
  SlidersHorizontal,
  X,
  PanelRight,
  PanelLeft,
  LayoutGrid,
  Rows3,
  Minus,
  Plus,
  Command as CommandIcon,
  Loader2,
} from "lucide-react";
import { useApp } from "../stores/useApp";
import {cx, basename, stripMarks, deckDisplayName } from "../lib/utils";
import FilterPopover from "./FilterPopover";
import SortMenu from "./SortMenu";

/** Unified titlebar toolbar (draggable) + the thin count/chips strip beneath.
 *  Interactive controls are marked `no-drag` and kept out of the traffic-light
 *  zone on the left. */
export default function Header() {
  const query = useApp((s) => s.query);
  const setQuery = useApp((s) => s.setQuery);
  const searching = useApp((s) => s.searching);
  const filters = useApp((s) => s.filters);
  const filterOpen = useApp((s) => s.filterPopoverOpen);
  const setFilterOpen = useApp((s) => s.setFilterPopoverOpen);
  const results = useApp((s) => s.results);
  const grouping = useApp((s) => s.grouping);
  const setGrouping = useApp((s) => s.setGrouping);
  const nav = useApp((s) => s.nav);
  const roots = useApp((s) => s.roots);
  const decks = useApp((s) => s.decks);

  const activeChips = countChips(filters);

  return (
    <header className="material hairline-b relative z-30 shrink-0">
      {/* Toolbar row — draggable window region. */}
      <div
        className="flex h-[52px] items-center gap-3 pr-3"
        data-tauri-drag-region
        style={{ paddingLeft: 92 }}
      >
        <div className="no-drag relative flex-1">
          <Search
            size={15}
            className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-subtle"
          />
          <input
            id="global-search"
            autoFocus
            spellCheck={false}
            autoComplete="off"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search every slide…"
            className="selectable h-8 w-full rounded-[7px] border border-hairline/10 bg-surface/80 pl-8 pr-8 text-body text-ink shadow-sm outline-none placeholder:text-subtle/70 focus:border-accent focus:bg-surface"
          />
          {searching ? (
            <Loader2
              size={14}
              className="absolute right-2.5 top-1/2 -translate-y-1/2 animate-spin text-subtle"
            />
          ) : query ? (
            <button
              className="absolute right-2 top-1/2 -translate-y-1/2 rounded-full p-0.5 text-subtle hover:bg-ink/10"
              onClick={() => setQuery("")}
              title="Clear (esc)"
            >
              <X size={13} />
            </button>
          ) : null}
        </div>

        <div className="no-drag relative flex items-center gap-1">
          <ToolbarBtn
            title="Filters"
            active={filterOpen || activeChips > 0}
            onClick={() => setFilterOpen(!filterOpen)}
          >
            <SlidersHorizontal size={15} />
            {activeChips > 0 && (
              <span className="tabnum ml-1 rounded-full bg-accent px-1 text-[10px] font-semibold text-white">
                {activeChips}
              </span>
            )}
          </ToolbarBtn>
          {filterOpen && <FilterPopover onClose={() => setFilterOpen(false)} />}
        </div>

        <div className="no-drag flex items-center gap-1">
          <ToolbarBtn
            title="Command palette (⌘K)"
            onClick={() => useApp.getState().setCommandOpen(true)}
          >
            <CommandIcon size={15} />
          </ToolbarBtn>
          <ToolbarBtn
            title="Toggle sidebar (⌘⌃S)"
            onClick={() => useApp.getState().toggleSidebar()}
          >
            <PanelLeft size={15} />
          </ToolbarBtn>
          <ToolbarBtn
            title="Toggle inspector (⌘I)"
            onClick={() => useApp.getState().toggleInspector()}
          >
            <PanelRight size={15} />
          </ToolbarBtn>
        </div>
      </div>

      {/* Count / chips / grouping / density strip. */}
      <div className="flex h-9 items-center gap-2 px-3 text-caption text-subtle">
        <span className="tabnum shrink-0">
          {query.trim()
            ? `${results.length} result${results.length === 1 ? "" : "s"}`
            : navLabel(nav, roots, decks, results.length)}
        </span>

        {/* Active filter chips */}
        <div className="flex flex-1 items-center gap-1.5 overflow-hidden">
          {query.trim() && (
            <Chip
              label={`“${truncate(query)}”`}
              onRemove={() => setQuery("")}
            />
          )}
          {filters.deck_query && (
            <Chip
              label={`Deck: ${filters.deck_query}`}
              onRemove={() =>
                useApp.getState().setFilters({ deck_query: null })
              }
            />
          )}
          {filters.path_prefix && (
            <Chip
              label={`Folder: ${basename(filters.path_prefix)}`}
              onRemove={() =>
                useApp.getState().setFilters({ path_prefix: null })
              }
            />
          )}
          {filters.modified_from && (
            <Chip
              label={`After ${fmt(filters.modified_from)}`}
              onRemove={() =>
                useApp.getState().setFilters({ modified_from: null })
              }
            />
          )}
          {filters.modified_to && (
            <Chip
              label={`Before ${fmt(filters.modified_to)}`}
              onRemove={() =>
                useApp.getState().setFilters({ modified_to: null })
              }
            />
          )}
        </div>

        {/* Density */}
        <div className="flex items-center overflow-hidden rounded-[6px] border border-hairline/10">
          <StripBtn
            title="Larger thumbnails (⌘+)"
            onClick={() => useApp.getState().decCols()}
          >
            <Plus size={12} />
          </StripBtn>
          <StripBtn
            title="Smaller thumbnails (⌘−)"
            onClick={() => useApp.getState().incCols()}
          >
            <Minus size={12} />
          </StripBtn>
        </div>

        {/* Sort */}
        <SortMenu />

        {/* Grouping toggle (1 / 2) */}
        <div className="flex items-center overflow-hidden rounded-[6px] border border-hairline/10">
          <StripBtn
            title="Flat (1)"
            active={grouping === "flat"}
            onClick={() => setGrouping("flat")}
          >
            <LayoutGrid size={12} />
          </StripBtn>
          <StripBtn
            title="Group by deck (2)"
            active={grouping === "deck"}
            onClick={() => setGrouping("deck")}
          >
            <Rows3 size={12} />
          </StripBtn>
        </div>
      </div>
    </header>
  );
}

function truncate(s: string, n = 24) {
  return s.length > n ? s.slice(0, n) + "…" : s;
}
function fmt(unix: number) {
  return new Date(unix * 1000).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}
function countChips(f: ReturnType<typeof useApp.getState>["filters"]) {
  let n = 0;
  if (f.deck_query) n += 1;
  if (f.path_prefix) n += 1;
  if (f.modified_from) n += 1;
  if (f.modified_to) n += 1;
  return n;
}
function navLabel(
  nav: ReturnType<typeof useApp.getState>["nav"],
  roots: ReturnType<typeof useApp.getState>["roots"],
  decks: ReturnType<typeof useApp.getState>["decks"],
  count: number,
) {
  const suffix = ` · ${count} slide${count === 1 ? "" : "s"}`;
  if (nav.type === "all") return "All Slides" + suffix;
  if (nav.type === "favorites") return "Favorites" + suffix;
  if (nav.type === "stats") return "Statistics";
  if (nav.type === "root") {
    const r = roots.find((x) => x.id === nav.id);
    return (r ? basename(r.path) : "Folder") + suffix;
  }
  const d = decks.find((x) => x.id === nav.id);
  return (d ? deckDisplayName(d) : "Deck") + suffix;
}

function ToolbarBtn({
  children,
  title,
  onClick,
  active,
}: {
  children: React.ReactNode;
  title: string;
  onClick: () => void;
  active?: boolean;
}) {
  return (
    <button
      title={title}
      onClick={onClick}
      className={cx(
        "flex h-8 items-center justify-center rounded-[6px] px-2 transition-colors",
        active ? "bg-accent/[0.14] text-accent" : "text-subtle hover:bg-ink/8 hover:text-ink",
      )}
    >
      {children}
    </button>
  );
}

function StripBtn({
  children,
  title,
  onClick,
  active,
}: {
  children: React.ReactNode;
  title: string;
  onClick: () => void;
  active?: boolean;
}) {
  return (
    <button
      title={title}
      onClick={onClick}
      className={cx(
        "flex h-6 w-7 items-center justify-center transition-colors",
        active ? "bg-accent text-white" : "text-subtle hover:bg-ink/8",
      )}
    >
      {children}
    </button>
  );
}

function Chip({ label, onRemove }: { label: string; onRemove: () => void }) {
  return (
    <span className="flex shrink-0 items-center gap-1 rounded-full bg-ink/8 py-0.5 pl-2 pr-1 text-caption text-ink">
      <span className="max-w-[160px] truncate">{stripMarks(label)}</span>
      <button
        onClick={onRemove}
        className="rounded-full p-0.5 text-subtle hover:bg-ink/15 hover:text-ink"
        title="Remove filter"
      >
        <X size={11} />
      </button>
    </span>
  );
}
