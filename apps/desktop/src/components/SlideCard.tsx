import { memo, useState } from "react";
import { motion } from "framer-motion";
import { Eye, Plus, FolderOpen, Check } from "lucide-react";
import type { SearchHit } from "../lib/types";
import { cx, prefersReducedMotion } from "../lib/utils";
import { useApp } from "../stores/useApp";
import { useTray } from "../stores/useTray";
import { toast } from "../stores/useToast";
import * as api from "../lib/api";
import Thumbnail from "./Thumbnail";
import ContextMenu, { type MenuItem } from "./ContextMenu";
import { DRAG_MIME, buildDragEntries, makeDragGhost } from "../lib/dnd";

interface SlideCardProps {
  hit: SearchHit;
  index: number;
}

function SlideCardImpl({ hit, index }: SlideCardProps) {
  const { slide, deck, snippet } = hit;
  const selected = useApp((s) => s.selectedIds.has(slide.id));
  const inTray = useTray((s) => s.items.some((i) => i.uid === `${slide.deck_id}:${slide.slide_index}`));
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const reduce = prefersReducedMotion();

  const onClick = (e: React.MouseEvent) => {
    const app = useApp.getState();
    if (e.metaKey || e.ctrlKey) app.toggleSelect(index);
    else if (e.shiftKey) app.rangeSelect(index);
    else {
      app.selectOnly(index);
      if (!app.inspectorVisible) app.setInspector(true);
    }
  };

  const addThis = () => {
    const added = useTray.getState().add([{ slide, deck }]);
    if (added > 0) {
      toast.success("Added to the tray");
      if (useTray.getState().collapsed) useTray.getState().setCollapsed(false);
    }
  };

  const onDragStart = (e: React.DragEvent) => {
    const app = useApp.getState();
    // If this card isn't in the current multiselection, drag just this one.
    const entries = app.selectedIds.has(slide.id)
      ? buildDragEntries(app.results, app.selectedIds)
      : [{ slide, deck }];
    e.dataTransfer.setData(DRAG_MIME, JSON.stringify(entries));
    e.dataTransfer.effectAllowed = "copy";
    const ghost = makeDragGhost(entries.length);
    e.dataTransfer.setDragImage(ghost, 24, 24);
    // Clean up the ghost node after the browser has snapshotted it.
    window.setTimeout(() => ghost.remove(), 0);
  };

  const menuItems: MenuItem[] = [
    { label: "Add to Tray", onClick: addThis },
    { label: "Peek", onClick: () => useApp.getState().openPeek(index) },
    {
      label: "Open source deck",
      onClick: () => void api.openFile(deck.path),
      separatorBefore: true,
    },
    { label: "Reveal in Finder", onClick: () => void api.revealInFinder(deck.path) },
    {
      label: "Copy source path",
      onClick: () => void navigator.clipboard?.writeText(deck.path),
      separatorBefore: true,
    },
    {
      label: "Find other slides from this deck",
      onClick: () => void useApp.getState().setNav({ type: "deck", id: deck.id }),
    },
  ];

  return (
    <>
      <div
        className="group relative select-none"
        draggable
        onDragStart={onDragStart}
        onClick={onClick}
        onDoubleClick={addThis}
        onContextMenu={(e) => {
          e.preventDefault();
          if (!selected) useApp.getState().selectOnly(index);
          setMenu({ x: e.clientX, y: e.clientY });
        }}
      >
        <motion.div
          layout={!reduce}
          whileHover={reduce ? undefined : { y: -2 }}
          transition={{ type: "spring", stiffness: 400, damping: 28 }}
          className={cx(
            "rounded-[8px] bg-surface p-1.5 shadow-tile transition-shadow duration-150 group-hover:shadow-tile-hover",
            selected && "ring-accent",
          )}
        >
          <div className="relative">
            <Thumbnail slideId={slide.id} alt={slide.title ?? deck.title} />

            {/* Hover caption: deck + slide number */}
            <div className="pointer-events-none absolute inset-x-0 bottom-0 flex items-center justify-between rounded-b-[6px] bg-gradient-to-t from-black/70 to-transparent px-2 pb-1 pt-4 text-caption text-white opacity-0 transition-opacity duration-150 group-hover:opacity-100">
              <span className="truncate">{deck.title}</span>
              <span className="tabnum ml-2 shrink-0 opacity-80">
                #{slide.slide_index}
              </span>
            </div>

            {/* Quick-action row (peek / add / reveal) */}
            <div className="absolute right-1.5 top-1.5 flex gap-1 opacity-0 transition-opacity duration-150 group-hover:opacity-100">
              <QuickBtn
                title="Peek (space)"
                onClick={(e) => {
                  e.stopPropagation();
                  useApp.getState().openPeek(index);
                }}
              >
                <Eye size={13} />
              </QuickBtn>
              <QuickBtn
                title={inTray ? "In tray" : "Add to tray"}
                active={inTray}
                onClick={(e) => {
                  e.stopPropagation();
                  addThis();
                }}
              >
                {inTray ? <Check size={13} /> : <Plus size={13} />}
              </QuickBtn>
              <QuickBtn
                title="Reveal in Finder"
                onClick={(e) => {
                  e.stopPropagation();
                  void api.revealInFinder(deck.path);
                }}
              >
                <FolderOpen size={13} />
              </QuickBtn>
            </div>

            {inTray && (
              <div className="absolute left-1.5 top-1.5 rounded-full bg-accent p-0.5 text-white shadow">
                <Check size={11} />
              </div>
            )}
          </div>

          {/* Text renders before the image decodes (readable-first). */}
          <div className="px-0.5 pb-0.5 pt-1.5">
            <div className="truncate text-body font-medium text-ink">
              {slide.title || deck.title}
            </div>
            <div
              className="mt-0.5 line-clamp-1 text-caption text-subtle [&_mark]:font-semibold"
              // Snippet is core-provided, escaped except for <mark> tags.
              dangerouslySetInnerHTML={{ __html: snippet || "&nbsp;" }}
            />
            <div className="mt-1 flex items-center gap-1 text-caption text-subtle/80">
              <span className="tabnum">Slide {slide.slide_index}</span>
              <span aria-hidden>·</span>
              <span className="truncate">{deck.file_name}</span>
            </div>
          </div>
        </motion.div>
      </div>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems}
          onClose={() => setMenu(null)}
        />
      )}
    </>
  );
}

function QuickBtn({
  children,
  title,
  onClick,
  active,
}: {
  children: React.ReactNode;
  title: string;
  onClick: (e: React.MouseEvent) => void;
  active?: boolean;
}) {
  return (
    <button
      title={title}
      onClick={onClick}
      className={cx(
        "no-drag flex h-6 w-6 items-center justify-center rounded-[5px] backdrop-blur-md transition-colors",
        active
          ? "bg-accent text-white"
          : "bg-black/45 text-white hover:bg-black/65",
      )}
    >
      {children}
    </button>
  );
}

export default memo(SlideCardImpl);
