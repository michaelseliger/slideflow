import { memo, useRef, useState } from "react";
import { Eye, Plus, FolderOpen, Check, Star } from "lucide-react";
import type { SearchHit, SlideDragPaths, SlidePick } from "../lib/types";
import { cx, deckDisplayName, prefersReducedMotion } from "../lib/utils";
import { useApp } from "../stores/useApp";
import { useTray } from "../stores/useTray";
import { useSemantic } from "../stores/useSemantic";
import { toast } from "../stores/useToast";
import * as api from "../lib/api";
import Thumbnail from "./Thumbnail";
import ApproxBadge from "./ApproxBadge";
import ContextMenu, { type MenuItem } from "./ContextMenu";
import { useSlidePreview } from "../lib/useSlideSvg";
import { DRAG_MIME, buildDragEntries, makeDragGhost } from "../lib/dnd";

interface SlideCardProps {
  hit: SearchHit;
  index: number;
}

// Native drag-out is macOS-first. If the drag plugin ever throws (e.g. an
// unsupported platform), we flip this once, warn the user, and stop attempting
// it — so a broken platform doesn't nag on every drag. Module-scoped so the
// decision is shared across all cards.
let nativeDragOff = false;

function SlideCardImpl({ hit, index }: SlideCardProps) {
  const { slide, deck, snippet } = hit;
  const selected = useApp((s) => s.selectedIds.has(slide.id));
  const inTray = useTray((s) => s.items.some((i) => i.uid === `${slide.deck_id}:${slide.slide_index}`));
  const semanticReady = useSemantic((s) => s.status?.state === "ready");
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  // Shares Thumbnail's (slide, "thumb") cache key, so no extra backend call.
  const { dropped } = useSlidePreview(slide.id, "thumb");
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

  // This card's slide as a SlidePick — the unit dragged out / saved.
  const pick: SlidePick = { pptx_path: deck.path, slide_index: slide.slide_index };
  // Holds an in-flight prepareSlideDrag so a ⌥-drag can reuse the pre-warmed
  // result and start the native drag promptly during the gesture.
  const prewarm = useRef<Promise<SlideDragPaths> | null>(null);

  // Kick off scratch-file prep so the .pptx is usually ready by `dragstart`.
  // Fired on ⌥-mousedown; cache-addressed on the backend, so priming a slide
  // whose files already exist is cheap.
  const primeDrag = () => {
    if (!api.isTauri() || nativeDragOff || prewarm.current) return;
    prewarm.current = api.prepareSlideDrag(pick).catch((err) => {
      prewarm.current = null; // let the next attempt retry from scratch
      throw err;
    });
  };

  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button === 0 && e.altKey) primeDrag();
  };

  // Drag this single slide out of the app as a real .pptx file.
  const startSlideDragOut = async () => {
    try {
      const ready = prewarm.current ?? api.prepareSlideDrag(pick);
      prewarm.current = null;
      const { pptx, icon } = await ready;
      await api.startNativeDrag([pptx], icon);
    } catch (err) {
      console.error("slide drag-out failed", err);
      if (!nativeDragOff) {
        nativeDragOff = true;
        toast.error("Drag out isn't available on this system");
      }
    }
  };

  const onDragStart = (e: React.DragEvent) => {
    // ⌥-drag → drag the slide out of the app as a real file (native shell only).
    // preventDefault suppresses the internal HTML5 drag so the two never race;
    // without ⌥, the existing grid → tray drag below is untouched.
    if (e.altKey && api.isTauri() && !nativeDragOff) {
      e.preventDefault();
      void startSlideDragOut();
      return;
    }
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

  // Context-menu "Save slide as .pptx…": compose this one slide to a
  // user-chosen path (all platforms; degrades to the mock in browser mode).
  const saveSlideAsPptx = async () => {
    const stem = deck.file_name.replace(/\.pptx$/i, "");
    const name = `${stem} — slide ${slide.slide_index}.pptx`;
    const cut = Math.max(deck.path.lastIndexOf("/"), deck.path.lastIndexOf("\\"));
    const dir = cut > 0 ? deck.path.slice(0, cut) : undefined;
    const dest = await api.pickSavePath(name, dir);
    if (!dest) return;
    try {
      await api.composeDeck([pick], dest, stem, false);
      toast.success("Saved the slide", {
        label: "Reveal",
        run: () => void api.revealInFinder(dest),
      });
    } catch (err) {
      console.error("save slide failed", err);
      toast.error("Couldn't save the slide");
    }
  };

  const menuItems: MenuItem[] = [
    { label: "Add to Tray", onClick: addThis },
    {
      label: slide.favorite ? "Remove from Favorites" : "Add to Favorites",
      onClick: () => void useApp.getState().toggleFavoriteSlide(slide.id),
    },
    {
      label: "Tags…",
      onClick: () => {
        // Select this slide and open the inspector, where the tag editor lives.
        useApp.getState().selectOnly(index);
        useApp.getState().setInspector(true);
      },
    },
    { label: "Peek", onClick: () => useApp.getState().openPeek(index) },
    {
      label: "Save slide as .pptx…",
      onClick: () => void saveSlideAsPptx(),
      separatorBefore: true,
    },
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
    // AI find-similar: only offered while the semantic model is ready. The
    // results render in the Inspector's "Similar slides" section.
    ...(semanticReady
      ? [
          {
            label: "Find similar (AI)",
            onClick: () => {
              const app = useApp.getState();
              app.selectOnly(index);
              app.setInspector(true);
            },
          },
        ]
      : []),
  ];

  return (
    <>
      <div
        className="group relative select-none"
        draggable
        onMouseDown={onMouseDown}
        onDragStart={onDragStart}
        onClick={onClick}
        onDoubleClick={addThis}
        onContextMenu={(e) => {
          e.preventDefault();
          if (!selected) useApp.getState().selectOnly(index);
          setMenu({ x: e.clientX, y: e.clientY });
        }}
      >
        <div
          className={cx(
            // Hover lift is a composited transform (cheap on WebView2); the
            // shadow swaps without animating (shadow transitions repaint). The
            // framer-motion `layout` prop was dropped — it forced a reflow on
            // every virtualized mount, which is constant during scroll.
            "rounded-[8px] bg-surface p-1.5 shadow-tile transition-transform duration-150 will-change-transform group-hover:shadow-tile-hover",
            !reduce && "group-hover:-translate-y-0.5",
            selected && "ring-accent",
          )}
        >
          <div className="relative">
            <Thumbnail slideId={slide.id} alt={slide.title ?? deckDisplayName(deck)} />

            {/* Hover caption: deck + slide number */}
            <div className="pointer-events-none absolute inset-x-0 bottom-0 flex items-center justify-between rounded-b-[6px] bg-gradient-to-t from-black/70 to-transparent px-2 pb-1 pt-4 text-caption text-white opacity-0 transition-opacity duration-150 group-hover:opacity-100">
              <span className="truncate">{deckDisplayName(deck)}</span>
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
                title={slide.favorite ? "Remove from Favorites" : "Add to Favorites"}
                onClick={(e) => {
                  e.stopPropagation();
                  void useApp.getState().toggleFavoriteSlide(slide.id);
                }}
              >
                <Star size={13} className={slide.favorite ? "fill-current text-amber-400" : ""} />
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

            {(inTray || slide.favorite || dropped.length > 0) && (
              <div className="absolute left-1.5 top-1.5 flex flex-col gap-1">
                {inTray && (
                  <div className="rounded-full bg-accent p-0.5 text-white shadow">
                    <Check size={11} />
                  </div>
                )}
                {slide.favorite && (
                  <div className="rounded-full bg-amber-400 p-0.5 text-white shadow">
                    <Star size={11} className="fill-current" />
                  </div>
                )}
                {dropped.length > 0 && <ApproxBadge dropped={dropped} variant="tile" />}
              </div>
            )}
          </div>

          {/* Text renders before the image decodes (readable-first). */}
          <div className="px-0.5 pb-0.5 pt-1.5">
            <div className="truncate text-body font-medium text-ink">
              {slide.title || deckDisplayName(deck)}
            </div>
            <div
              className="mt-0.5 line-clamp-1 text-caption text-subtle [&_mark]:font-semibold"
              // Snippet is core-provided, escaped except for <mark> tags.
              dangerouslySetInnerHTML={{ __html: snippet || "&nbsp;" }}
            />
            <div className="mt-1 flex items-center gap-1 text-caption text-subtle/80">
              <span className="tabnum">Slide {slide.slide_index}</span>
              <span aria-hidden>·</span>
              <span className="truncate" title={deck.path}>
                {deck.file_name}
              </span>
            </div>
          </div>
        </div>
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
        // Solid translucent bg instead of backdrop-blur — backdrop-filter forces
        // an expensive compositing layer, notably weaker on WebView2.
        "no-drag flex h-6 w-6 items-center justify-center rounded-[5px] transition-colors",
        active
          ? "bg-accent text-white"
          : "bg-black/60 text-white hover:bg-black/75",
      )}
    >
      {children}
    </button>
  );
}

export default memo(SlideCardImpl);
