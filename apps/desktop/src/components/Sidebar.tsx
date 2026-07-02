import { useState } from "react";
import {
  Layers,
  Folder,
  FolderPlus,
  Presentation,
  Loader2,
  Star,
  BarChart3,
  Info,
} from "lucide-react";
import { useApp } from "../stores/useApp";
import { cx, basename, deckDisplayName } from "../lib/utils";
import ContextMenu, { type MenuItem } from "./ContextMenu";
import * as api from "../lib/api";

/** Left source tree: All Slides root, per-folder roots with live counts, the
 *  decks list, a bottom-pinned "Add folder…" row, and the live scan progress.
 *  Rendered on a vibrancy material per the HIG. */
export default function Sidebar() {
  const collapsed = useApp((s) => s.sidebarCollapsed);
  const roots = useApp((s) => s.roots);
  const decks = useApp((s) => s.decks);
  const stats = useApp((s) => s.stats);
  const nav = useApp((s) => s.nav);
  const scan = useApp((s) => s.scan);
  const setNav = useApp((s) => s.setNav);
  const addFolder = useApp((s) => s.addFolder);
  const [menu, setMenu] = useState<{ x: number; y: number; rootId: number } | null>(null);
  const [deckMenu, setDeckMenu] = useState<{ x: number; y: number; deckId: number } | null>(
    null,
  );

  const isActive = (type: string, id?: number) =>
    nav.type === type && nav.id === id;

  const width = collapsed ? 60 : 224;

  return (
    <aside
      className="material hairline-r relative flex shrink-0 flex-col"
      style={{ width }}
    >
      {/* Space for the traffic lights + drag region at the very top. */}
      <div className="h-[52px] shrink-0" data-tauri-drag-region />

      <nav className="flex-1 overflow-y-auto px-2 pb-2">
        {!collapsed && <SectionLabel>Library</SectionLabel>}

        <Row
          icon={<Layers size={15} />}
          label="All Slides"
          count={stats.slide_count}
          active={isActive("all")}
          collapsed={collapsed}
          onClick={() => void setNav({ type: "all" })}
        />

        <Row
          icon={<Star size={15} />}
          label="Favorites"
          active={isActive("favorites")}
          collapsed={collapsed}
          onClick={() => void setNav({ type: "favorites" })}
        />

        <Row
          icon={<BarChart3 size={15} />}
          label="Statistics"
          active={isActive("stats")}
          collapsed={collapsed}
          onClick={() => void setNav({ type: "stats" })}
        />

        {roots.map((r) => (
          <Row
            key={r.id}
            icon={<Folder size={15} />}
            label={basename(r.path)}
            count={r.slide_count}
            active={isActive("root", r.id)}
            collapsed={collapsed}
            onClick={() => void setNav({ type: "root", id: r.id })}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu({ x: e.clientX, y: e.clientY, rootId: r.id });
            }}
          />
        ))}

        {decks.length > 0 && (
          <>
            {!collapsed && <SectionLabel>Decks</SectionLabel>}
            {decks.map((d) => (
              <Row
                key={d.id}
                icon={
                  d.favorite ? (
                    <Star size={15} className="fill-current text-amber-400" />
                  ) : (
                    <Presentation size={15} />
                  )
                }
                label={deckDisplayName(d)}
                tooltip={d.path}
                count={d.slide_count}
                active={isActive("deck", d.id)}
                collapsed={collapsed}
                onClick={() => void setNav({ type: "deck", id: d.id })}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setDeckMenu({ x: e.clientX, y: e.clientY, deckId: d.id });
                }}
              />
            ))}
          </>
        )}
      </nav>

      {/* Live scan progress. */}
      {scan.running && !collapsed && (
        <div className="mx-2 mb-1 rounded-[6px] bg-ink/5 px-2.5 py-2">
          <div className="flex items-center gap-1.5 text-caption text-subtle">
            <Loader2 size={12} className="animate-spin" />
            <span className="truncate">
              Indexing… <span className="tabnum">{scan.indexed}</span> slides
            </span>
          </div>
          <div className="mt-1.5 h-1 overflow-hidden rounded-full bg-ink/10">
            <div
              className="h-full rounded-full bg-accent transition-[width] duration-300"
              style={{
                width: scan.total
                  ? `${Math.min(100, (scan.done / scan.total) * 100)}%`
                  : "35%",
              }}
            />
          </div>
        </div>
      )}
      {scan.running && collapsed && (
        <div className="flex justify-center pb-2">
          <Loader2 size={16} className="animate-spin text-accent" />
        </div>
      )}

      {/* Pinned Add-folder row. */}
      <button
        onClick={() => void addFolder()}
        className={cx(
          "hairline-t flex items-center gap-2 px-4 py-2.5 text-body text-subtle transition-colors hover:bg-ink/5 hover:text-ink",
          collapsed && "justify-center px-0",
        )}
        title="Add folder…"
      >
        <FolderPlus size={15} />
        {!collapsed && <span>Add folder…</span>}
      </button>

      {/* Pinned About row. */}
      <button
        onClick={() => useApp.getState().setAboutOpen(true)}
        className={cx(
          "flex items-center gap-2 px-4 py-2.5 text-body text-subtle transition-colors hover:bg-ink/5 hover:text-ink",
          collapsed && "justify-center px-0",
        )}
        title="About Slideflow"
      >
        <Info size={15} />
        {!collapsed && <span>About</span>}
      </button>

      {deckMenu && (
        <ContextMenu
          x={deckMenu.x}
          y={deckMenu.y}
          onClose={() => setDeckMenu(null)}
          items={(() => {
            const deck = decks.find((d) => d.id === deckMenu.deckId);
            return [
              {
                label: deck?.favorite ? "Remove from Favorites" : "Add to Favorites",
                onClick: () =>
                  void useApp.getState().toggleFavoriteDeck(deckMenu.deckId),
              },
              {
                label: "Reveal in Finder",
                onClick: () => {
                  if (deck) void api.revealInFinder(deck.path);
                },
              },
            ] as MenuItem[];
          })()}
        />
      )}

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={
            [
              {
                label: "Re-index this folder",
                onClick: () => void useApp.getState().startScan(),
              },
              {
                label: "Reveal in Finder",
                onClick: () => {
                  const r = roots.find((x) => x.id === menu.rootId);
                  if (r) void api.revealInFinder(r.path);
                },
              },
              {
                label: "Remove folder",
                danger: true,
                separatorBefore: true,
                onClick: () =>
                  void useApp.getState().removeRoot(menu.rootId),
              },
            ] as MenuItem[]
          }
        />
      )}
    </aside>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="px-2 pb-1 pt-3 text-caption font-semibold uppercase tracking-wide text-subtle/70">
      {children}
    </div>
  );
}

function Row({
  icon,
  label,
  tooltip,
  count,
  active,
  collapsed,
  onClick,
  onContextMenu,
}: {
  icon: React.ReactNode;
  label: string;
  /** Hover tooltip (e.g. the full path); falls back to the label when collapsed. */
  tooltip?: string;
  count?: number;
  active: boolean;
  collapsed: boolean;
  onClick: () => void;
  onContextMenu?: (e: React.MouseEvent) => void;
}) {
  return (
    <button
      onClick={onClick}
      onContextMenu={onContextMenu}
      title={tooltip ?? (collapsed ? label : undefined)}
      className={cx(
        "group mb-0.5 flex w-full items-center gap-2 rounded-[6px] px-2 py-1.5 text-body transition-colors",
        collapsed && "justify-center px-0",
        active
          ? "bg-accent/[0.14] text-accent"
          : "text-ink hover:bg-ink/5",
      )}
    >
      <span className={cx("shrink-0", active ? "text-accent" : "text-subtle")}>
        {icon}
      </span>
      {!collapsed && (
        <>
          <span className="flex-1 truncate text-left">{label}</span>
          {count != null && (
            <span className="tabnum text-caption text-subtle/80">{count}</span>
          )}
        </>
      )}
    </button>
  );
}
