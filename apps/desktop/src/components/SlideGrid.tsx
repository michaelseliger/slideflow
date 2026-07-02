import { useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronDown, ChevronRight, FolderOpen } from "lucide-react";
import type { SearchHit } from "../lib/types";
import { useApp } from "../stores/useApp";
import { cx, deckDisplayName } from "../lib/utils";
import * as api from "../lib/api";
import SlideCard from "./SlideCard";

const GAP = 18; // brief: generous 16–20px gutters
const PAD = 20;
const TEXT_BLOCK = 74; // title + snippet + meta + paddings under the thumb
const HEADER_H = 44;

interface IndexedHit {
  hit: SearchHit;
  index: number;
}

type Row =
  | { kind: "header"; deckId: number; title: string; path: string; count: number }
  | { kind: "cards"; cells: IndexedHit[] };

/** The virtualized results/browse grid. Switches seamlessly between flat
 *  relevance order and group-by-deck without unmounting the scroller. */
export default function SlideGrid() {
  const results = useApp((s) => s.results);
  const grouping = useApp((s) => s.grouping);
  const cols = useApp((s) => s.gridCols);

  const parentRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(1000);
  const [collapsed, setCollapsed] = useState<Set<number>>(new Set());

  // Measure available width to size cards + compute row heights.
  useEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width ?? el.clientWidth;
      setWidth(w);
    });
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  const colWidth = Math.max(
    120,
    (width - PAD * 2 - GAP * (cols - 1)) / cols,
  );
  const cardHeight = (colWidth * 9) / 16 + TEXT_BLOCK;
  const rowHeight = cardHeight + GAP;

  // Build the flattened row model.
  const rows = useMemo<Row[]>(() => {
    const indexed: IndexedHit[] = results.map((hit, index) => ({ hit, index }));
    if (grouping === "flat") {
      const out: Row[] = [];
      for (let i = 0; i < indexed.length; i += cols) {
        out.push({ kind: "cards", cells: indexed.slice(i, i + cols) });
      }
      return out;
    }
    // Group by deck, preserving first-seen order.
    const groups = new Map<number, IndexedHit[]>();
    const order: number[] = [];
    for (const cell of indexed) {
      const id = cell.hit.deck.id;
      if (!groups.has(id)) {
        groups.set(id, []);
        order.push(id);
      }
      groups.get(id)!.push(cell);
    }
    const out: Row[] = [];
    for (const id of order) {
      const cells = groups.get(id)!;
      const deck = cells[0].hit.deck;
      out.push({
        kind: "header",
        deckId: id,
        title: deckDisplayName(deck),
        path: deck.path,
        count: cells.length,
      });
      if (!collapsed.has(id)) {
        for (let i = 0; i < cells.length; i += cols) {
          out.push({ kind: "cards", cells: cells.slice(i, i + cols) });
        }
      }
    }
    return out;
  }, [results, grouping, cols, collapsed]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: (i) => (rows[i].kind === "header" ? HEADER_H : rowHeight),
    overscan: 4,
    getItemKey: (i) => {
      const r = rows[i];
      if (r.kind === "header") return `h-${r.deckId}`;
      return `r-${r.cells[0]?.index ?? i}`;
    },
  });

  // Re-measure when the row height changes (cols/width) so virtual offsets stay
  // correct.
  useEffect(() => {
    virtualizer.measure();
  }, [rowHeight, virtualizer]);

  const toggleGroup = (deckId: number) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(deckId)) next.delete(deckId);
      else next.add(deckId);
      return next;
    });

  return (
    <div
      ref={parentRef}
      className="h-full overflow-auto overflow-x-hidden bg-canvas"
      style={{ scrollbarGutter: "stable" }}
    >
      <div
        className="relative w-full"
        style={{ height: virtualizer.getTotalSize(), paddingTop: 4 }}
      >
        {virtualizer.getVirtualItems().map((vi) => {
          const row = rows[vi.index];
          return (
            <div
              key={vi.key}
              className="absolute left-0 top-0 w-full"
              style={{ transform: `translateY(${vi.start}px)`, height: vi.size }}
            >
              {row.kind === "header" ? (
                <GroupHeader
                  title={row.title}
                  path={row.path}
                  count={row.count}
                  collapsed={collapsed.has(row.deckId)}
                  onToggle={() => toggleGroup(row.deckId)}
                />
              ) : (
                <div
                  className="grid"
                  style={{
                    gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
                    gap: GAP,
                    paddingLeft: PAD,
                    paddingRight: PAD,
                  }}
                >
                  {row.cells.map((cell) => (
                    <SlideCard
                      key={cell.hit.slide.id}
                      hit={cell.hit}
                      index={cell.index}
                    />
                  ))}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function GroupHeader({
  title,
  path,
  count,
  collapsed,
  onToggle,
}: {
  title: string;
  path: string;
  count: number;
  collapsed: boolean;
  onToggle: () => void;
}) {
  return (
    <div
      className="flex items-center gap-1.5 px-5 pb-1.5 pt-3"
      style={{ height: HEADER_H }}
    >
      <button
        onClick={onToggle}
        className="flex items-center gap-1.5 rounded-[5px] px-1 py-0.5 hover:bg-ink/5"
      >
        {collapsed ? (
          <ChevronRight size={14} className="text-subtle" />
        ) : (
          <ChevronDown size={14} className="text-subtle" />
        )}
        <span className="text-title font-semibold text-ink" title={path}>
          {title}
        </span>
        <span className="tabnum ml-1 rounded-full bg-ink/8 px-1.5 text-caption text-subtle">
          {count}
        </span>
      </button>
      <button
        title="Reveal deck in Finder"
        onClick={() => void api.revealInFinder(path)}
        className={cx(
          "ml-1 flex h-6 w-6 items-center justify-center rounded-[5px] text-subtle opacity-0 hover:bg-ink/5 hover:opacity-100",
          "group-hover:opacity-100",
        )}
      >
        <FolderOpen size={13} />
      </button>
      <span className="ml-auto truncate pr-2 text-caption text-subtle/70">
        {path}
      </span>
    </div>
  );
}
