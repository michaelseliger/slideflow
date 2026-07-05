import { useRef, useState } from "react";
import { Reorder, AnimatePresence, motion } from "framer-motion";
import { X, ChevronDown, Download, Layers, AlertTriangle, Trash2, Ratio } from "lucide-react";
import { useTray, type TrayItem } from "../stores/useTray";
import { useApp } from "../stores/useApp";
import { toast } from "../stores/useToast";
import { cx, prefersReducedMotion } from "../lib/utils";
import { parseDropEntries } from "../lib/dnd";
import { mismatchedDimUids } from "../lib/trayDims";
import Thumbnail from "./Thumbnail";

const EXPANDED_H = 132;
const COLLAPSED_H = 34;
const spring = { type: "spring" as const, stiffness: 300, damping: 30 };

/** Bottom-docked composition tray: a persistent, ordered filmstrip with
 *  drag-reorder, drag-in from the grid, remove-on-hover, and Export. */
export default function Tray() {
  const items = useTray((s) => s.items);
  const collapsed = useTray((s) => s.collapsed);
  const reorder = useTray((s) => s.reorder);
  const [dragOver, setDragOver] = useState(false);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const reduce = prefersReducedMotion();

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);
    const entries = parseDropEntries(e.dataTransfer);
    if (entries.length === 0) return;
    // Insertion index from cursor x within the scroller.
    let atIndex: number | undefined;
    const scroller = scrollerRef.current;
    if (scroller) {
      const kids = Array.from(
        scroller.querySelectorAll<HTMLElement>("[data-tray-item]"),
      );
      const x = e.clientX;
      atIndex = kids.length;
      for (let i = 0; i < kids.length; i += 1) {
        const r = kids[i].getBoundingClientRect();
        if (x < r.left + r.width / 2) {
          atIndex = i;
          break;
        }
      }
    }
    const added = useTray.getState().add(entries, atIndex);
    if (added > 0) {
      toast.success(added === 1 ? "Added to the tray" : `Added ${added} slides`);
      if (collapsed) useTray.getState().setCollapsed(false);
    }
  };

  const height = collapsed ? COLLAPSED_H : EXPANDED_H;
  const movedCount = items.filter((i) => i.moved).length;
  const mismatched = mismatchedDimUids(items);
  const mismatchCount = mismatched.size;

  return (
    <motion.section
      className="material hairline-t relative z-20 flex shrink-0 flex-col"
      initial={false}
      animate={{ height }}
      transition={reduce ? { duration: 0 } : spring}
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes("application/x-slideflow-slides")) {
          e.preventDefault();
          e.dataTransfer.dropEffect = "copy";
          if (!dragOver) setDragOver(true);
        }
      }}
      onDragLeave={(e) => {
        if (e.currentTarget === e.target) setDragOver(false);
      }}
      onDrop={onDrop}
    >
      {/* Header bar */}
      <div className="flex h-8 shrink-0 items-center gap-2 px-3">
        <button
          className="flex items-center gap-1.5 rounded-[5px] px-1 py-0.5 text-body font-medium text-ink hover:bg-ink/5"
          onClick={() => useTray.getState().toggleCollapsed()}
          title="Toggle tray (⌘T)"
        >
          <Layers size={14} className="text-accent" />
          Composition
          <span className="tabnum rounded-full bg-accent px-1.5 text-caption font-semibold text-white">
            {items.length}
          </span>
          <ChevronDown
            size={13}
            className={cx(
              "text-subtle transition-transform",
              collapsed && "rotate-180",
            )}
          />
        </button>

        {movedCount > 0 && (
          <span
            className="flex items-center gap-1 text-caption text-amber-500"
            title="Some source decks moved or changed since they were added"
          >
            <AlertTriangle size={12} />
            {movedCount} moved
          </span>
        )}

        {mismatchCount > 0 && (
          <span
            className="flex items-center gap-1 text-caption text-amber-500"
            title="Some picks use a different slide size than the first pick — they may not fit the exported deck"
          >
            <Ratio size={12} />
            {mismatchCount} off-size
          </span>
        )}

        <div className="ml-auto flex items-center gap-1.5">
          {items.length > 0 && (
            <button
              className="flex items-center gap-1 rounded-[5px] px-2 py-1 text-caption text-subtle hover:bg-ink/5 hover:text-ink"
              onClick={() => useTray.getState().clear()}
              title="Clear tray"
            >
              <Trash2 size={12} /> Clear
            </button>
          )}
          <button
            className="flex items-center gap-1.5 rounded-[6px] bg-accent px-3 py-1 text-caption font-semibold text-white transition-opacity hover:opacity-90 disabled:opacity-40"
            disabled={items.length === 0}
            onClick={() => openExport()}
            title="Export… (⌘E)"
          >
            <Download size={13} />
            Export…
          </button>
        </div>
      </div>

      {/* Filmstrip / empty state */}
      {!collapsed && (
        <div className="min-h-0 flex-1 px-3 pb-2">
          {items.length === 0 ? (
            <EmptyTray dragOver={dragOver} />
          ) : (
            <Reorder.Group
              ref={scrollerRef}
              axis="x"
              values={items}
              onReorder={reorder}
              className="flex h-full items-center gap-2 overflow-x-auto overflow-y-hidden pb-1"
            >
              <AnimatePresence initial={false}>
                {items.map((item, index) => (
                  <TrayThumb
                    key={item.uid}
                    item={item}
                    index={index}
                    reduce={reduce}
                    dimsMismatch={mismatched.has(item.uid)}
                  />
                ))}
              </AnimatePresence>
            </Reorder.Group>
          )}
        </div>
      )}

      {/* Collapsed drop hint overlay */}
      {collapsed && dragOver && (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center bg-accent/10 text-caption font-medium text-accent">
          Drop to add to your deck
        </div>
      )}
    </motion.section>
  );
}

function openExport() {
  useApp.getState().setCommandOpen(false);
  useApp.getState().setExportOpen(true);
}

function TrayThumb({
  item,
  index,
  reduce,
  dimsMismatch,
}: {
  item: TrayItem;
  index: number;
  reduce: boolean;
  dimsMismatch: boolean;
}) {
  return (
    <Reorder.Item
      value={item}
      data-tray-item
      className="group relative h-full shrink-0"
      whileDrag={{ scale: 1.05, zIndex: 10 }}
      transition={reduce ? { duration: 0 } : spring}
      initial={reduce ? false : { opacity: 0, scale: 0.8 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={reduce ? { opacity: 0 } : { opacity: 0, scale: 0.8, y: 12 }}
      style={{ aspectRatio: "16 / 9" }}
    >
      <div
        className={cx(
          "relative h-full overflow-hidden rounded-[6px] bg-white shadow-tile ring-1 ring-black/10",
          item.moved && "ring-2 ring-amber-400",
        )}
      >
        <Thumbnail slideId={item.slide.id} rounded={false} />
        <span className="tabnum absolute bottom-0.5 left-0.5 rounded bg-black/55 px-1 text-[10px] text-white">
          {index + 1}
        </span>
        {item.moved && (
          <span
            className="absolute right-0.5 top-0.5 rounded bg-amber-400 p-0.5 text-black"
            title="Source deck moved or changed"
          >
            <AlertTriangle size={10} />
          </span>
        )}
        {dimsMismatch && (
          <span
            className="absolute left-0.5 top-0.5 rounded bg-amber-400 p-0.5 text-black"
            title="Different slide size than the first pick — may not fit the exported deck"
          >
            <Ratio size={10} />
          </span>
        )}
      </div>
      <button
        className="absolute -right-1.5 -top-1.5 z-10 flex h-5 w-5 items-center justify-center rounded-full bg-ink text-white opacity-0 shadow transition-opacity group-hover:opacity-100"
        onClick={() => useTray.getState().remove(item.uid)}
        title="Remove"
      >
        <X size={12} />
      </button>
    </Reorder.Item>
  );
}

function EmptyTray({ dragOver }: { dragOver: boolean }) {
  return (
    <div
      className={cx(
        "flex h-full items-center justify-center rounded-[8px] border-2 border-dashed transition-colors",
        dragOver
          ? "border-accent bg-accent/10 text-accent"
          : "border-hairline/15 text-subtle",
      )}
    >
      <span className="text-caption">
        Drag slides here to build a deck — they keep their original formatting.
      </span>
    </div>
  );
}
