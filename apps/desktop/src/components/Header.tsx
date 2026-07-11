import { useState } from "react";
import {
  Search,
  X,
  PanelRight,
  PanelLeft,
  LayoutGrid,
  Rows3,
  Minus,
  Plus,
  Command as CommandIcon,
  Loader2,
  HelpCircle,
  Bookmark,
  Sparkles,
  SunMoon,
} from "lucide-react";
import { useApp } from "../stores/useApp";
import { useSemantic } from "../stores/useSemantic";
import {cx, basename, stripMarks, deckDisplayName } from "../lib/utils";
import SearchHelpPopover from "./SearchHelpPopover";
import SaveSearchPopover from "./SaveSearchPopover";
import SortMenu from "./SortMenu";

/** Unified titlebar toolbar (draggable) + the thin count/chips strip beneath.
 *  Interactive controls are marked `no-drag` and kept out of the traffic-light
 *  zone on the left. */
export default function Header() {
  const query = useApp((s) => s.query);
  const setQuery = useApp((s) => s.setQuery);
  const searching = useApp((s) => s.searching);
  const results = useApp((s) => s.results);
  const grouping = useApp((s) => s.grouping);
  const setGrouping = useApp((s) => s.setGrouping);
  const nav = useApp((s) => s.nav);
  const roots = useApp((s) => s.roots);
  const decks = useApp((s) => s.decks);
  const savedSearches = useApp((s) => s.savedSearches);
  const tags = useApp((s) => s.tags);
  const [helpOpen, setHelpOpen] = useState(false);
  const [saveOpen, setSaveOpen] = useState(false);
  const searchMode = useApp((s) => s.searchMode);
  const setSearchMode = useApp((s) => s.setSearchMode);
  const semanticReady = useSemantic((s) => s.status?.state === "ready");
  const sidebarCollapsed = useApp((s) => s.sidebarCollapsed);
  const inspectorVisible = useApp((s) => s.inspectorVisible);

  // The density / grouping / sort cluster only applies to a browsable slide
  // grid — hide it on the stats and duplicates surfaces, which own their layout.
  const showGridControls = nav.type !== "stats" && nav.type !== "duplicates";

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
            className="selectable h-8 w-full rounded-[7px] border border-hairline/10 bg-surface/80 pl-8 pr-14 text-body text-ink shadow-xs outline-hidden placeholder:text-subtle/70 focus:border-accent focus:bg-surface"
          />
          {searching ? (
            <Loader2
              size={14}
              className="absolute right-8 top-1/2 -translate-y-1/2 animate-spin text-subtle"
            />
          ) : query ? (
            <button
              className="absolute right-8 top-1/2 -translate-y-1/2 rounded-full p-0.5 text-subtle hover:bg-ink/10"
              onClick={() => setQuery("")}
              title="Clear (esc)"
            >
              <X size={13} />
            </button>
          ) : null}
          <button
            className={cx(
              "absolute right-2 top-1/2 -translate-y-1/2 rounded-full p-0.5 transition-colors",
              helpOpen ? "text-accent" : "text-subtle hover:bg-ink/10 hover:text-ink",
            )}
            onClick={() => setHelpOpen((v) => !v)}
            title="Search syntax"
            aria-label="Search syntax help"
          >
            <HelpCircle size={14} />
          </button>
          {helpOpen && <SearchHelpPopover onClose={() => setHelpOpen(false)} />}
        </div>

        {query.trim() && (
          <div className="no-drag relative flex items-center gap-1">
            <ToolbarBtn
              title="Save this search"
              active={saveOpen}
              onClick={() => setSaveOpen((v) => !v)}
            >
              <Bookmark size={15} />
            </ToolbarBtn>
            {saveOpen && <SaveSearchPopover onClose={() => setSaveOpen(false)} />}
          </div>
        )}

        {/* Retrieval-mode toggle — only once the semantic model is ready. */}
        {semanticReady && (
          <div className="no-drag flex items-center overflow-hidden rounded-[6px] border border-hairline/10">
            <ModeBtn
              title="Exact — match the words you type"
              active={searchMode === "lexical"}
              onClick={() => setSearchMode("lexical")}
            >
              <span className="text-[11px] font-semibold">Aa</span>
            </ModeBtn>
            <ModeBtn
              title="Semantic — match by meaning"
              active={searchMode === "semantic"}
              onClick={() => setSearchMode("semantic")}
            >
              <Sparkles size={12} />
            </ModeBtn>
            <ModeBtn
              title="Hybrid — exact and semantic combined"
              active={searchMode === "hybrid"}
              onClick={() => setSearchMode("hybrid")}
            >
              <span className="flex items-center gap-0.5 text-[11px] font-semibold">
                Aa
                <Sparkles size={10} />
              </span>
            </ModeBtn>
          </div>
        )}

        <div className="no-drag flex items-center gap-1">
          <ToolbarBtn
            title="Command palette (⌘K)"
            onClick={() => useApp.getState().setCommandOpen(true)}
          >
            <CommandIcon size={15} />
          </ToolbarBtn>
          <ToolbarBtn
            title="Theme (system / light / dark)"
            onClick={() => useApp.getState().cycleTheme()}
          >
            <SunMoon size={15} />
          </ToolbarBtn>
          <ToolbarBtn
            title="Toggle sidebar (⌘⌃S)"
            active={!sidebarCollapsed}
            onClick={() => useApp.getState().toggleSidebar()}
          >
            <PanelLeft size={15} />
          </ToolbarBtn>
          <ToolbarBtn
            title="Toggle inspector (⌘I)"
            active={inspectorVisible}
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
            : navLabel(nav, roots, decks, savedSearches, tags, results.length)}
        </span>

        {/* Active query chip */}
        <div className="flex flex-1 items-center gap-1.5 overflow-hidden">
          {query.trim() && (
            <Chip
              label={`“${truncate(query)}”`}
              onRemove={() => setQuery("")}
            />
          )}
        </div>

        {showGridControls && (
          <div className="flex items-center gap-2">
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

            {/* Sort */}
            <SortMenu />
          </div>
        )}
      </div>
    </header>
  );
}

function truncate(s: string, n = 24) {
  return s.length > n ? s.slice(0, n) + "…" : s;
}
function navLabel(
  nav: ReturnType<typeof useApp.getState>["nav"],
  roots: ReturnType<typeof useApp.getState>["roots"],
  decks: ReturnType<typeof useApp.getState>["decks"],
  savedSearches: ReturnType<typeof useApp.getState>["savedSearches"],
  tags: ReturnType<typeof useApp.getState>["tags"],
  count: number,
) {
  const suffix = ` · ${count} slide${count === 1 ? "" : "s"}`;
  if (nav.type === "all") return "All Slides" + suffix;
  if (nav.type === "favorites") return "Favorites" + suffix;
  if (nav.type === "stats") return "Statistics";
  if (nav.type === "saved") {
    const s = savedSearches.find((x) => x.id === nav.id);
    return (s ? s.name : "Saved Search") + suffix;
  }
  if (nav.type === "duplicates") return "Duplicates";
  if (nav.type === "root") {
    const r = roots.find((x) => x.id === nav.id);
    return (r ? basename(r.path) : "Folder") + suffix;
  }
  if (nav.type === "tag") {
    const t = tags.find((x) => x.id === nav.id);
    return (t ? `#${t.name}` : "Tag") + suffix;
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

/** Toolbar-height segmented-control button for the retrieval-mode toggle. */
function ModeBtn({
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
        "flex h-8 w-9 items-center justify-center transition-colors",
        active ? "bg-accent text-white" : "text-subtle hover:bg-ink/8",
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
