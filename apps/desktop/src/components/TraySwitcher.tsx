import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, Layers, Pencil, Plus, Trash2 } from "lucide-react";
import { useTray } from "../stores/useTray";
import { useApp } from "../stores/useApp";
import { cx } from "../lib/utils";
import { useDismiss } from "../lib/useDismiss";

/** Tray switcher for the composition header: a compact dropdown listing every
 *  named tray with its slide count, plus create / inline-rename / delete. Opens
 *  UPWARD (the tray is docked to the bottom of the window). Self-contained open
 *  state, mirroring `SortMenu`. */
export default function TraySwitcher() {
  const activeId = useTray((s) => s.activeId);
  const order = useTray((s) => s.order);
  const trays = useTray((s) => s.trays);

  const [open, setOpen] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const ref = useRef<HTMLDivElement>(null);

  useDismiss(ref, () => setOpen(false), { enabled: open });

  // Reset any in-progress rename when the menu closes.
  useEffect(() => {
    if (!open) {
      setEditingId(null);
      setDraft("");
    }
  }, [open]);

  const active = trays[activeId];
  const list = order.map((id) => ({
    id,
    name: trays[id]?.name ?? "Tray",
    count: trays[id]?.items.length ?? 0,
  }));

  const startRename = (id: string, name: string) => {
    setEditingId(id);
    setDraft(name);
  };
  const commitRename = () => {
    if (editingId) useTray.getState().renameTray(editingId, draft);
    setEditingId(null);
    setDraft("");
  };
  const cancelRename = () => {
    setEditingId(null);
    setDraft("");
  };

  const confirmDelete = (id: string, name: string, count: number) => {
    setOpen(false);
    useApp.getState().requestConfirm({
      title: "Delete tray?",
      message: `“${name}” and its ${count} slide${count === 1 ? "" : "s"} will be removed. This can't be undone.`,
      confirmLabel: "Delete tray",
      destructive: true,
      onConfirm: () => useTray.getState().deleteTray(id),
    });
  };

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        className={cx(
          "flex items-center gap-1.5 rounded-[5px] px-1 py-0.5 text-body font-medium text-ink hover:bg-ink/5",
          open && "bg-ink/5",
        )}
        title="Switch tray"
      >
        <Layers size={14} className="text-accent" />
        <span className="max-w-[160px] truncate">{active?.name ?? "Composition"}</span>
        <span className="tabnum rounded-full bg-accent px-1.5 text-caption font-semibold text-white">
          {active?.items.length ?? 0}
        </span>
        <ChevronDown
          size={13}
          className={cx("text-subtle transition-transform", open && "rotate-180")}
        />
      </button>

      {open && (
        <div className="absolute bottom-full left-0 z-50 mb-1.5 w-64 rounded-[8px] border border-hairline/10 bg-elevated p-1 shadow-peek">
          <div className="px-2 py-1 text-caption font-medium text-subtle">Trays</div>
          {list.map((t) => (
            <div
              key={t.id}
              className={cx(
                "group/tray flex items-center gap-0.5 rounded-[6px] pr-1",
                t.id === activeId ? "bg-accent/[0.14]" : "hover:bg-ink/8",
              )}
            >
              {editingId === t.id ? (
                <input
                  autoFocus
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      commitRename();
                    } else if (e.key === "Escape") {
                      e.preventDefault();
                      e.stopPropagation();
                      cancelRename();
                    }
                  }}
                  onBlur={commitRename}
                  className="my-0.5 ml-1 h-6 flex-1 rounded-[4px] border border-hairline/20 bg-canvas px-1.5 text-body text-ink outline-none focus:border-accent"
                />
              ) : (
                <>
                  <button
                    onClick={() => {
                      useTray.getState().switchTray(t.id);
                      setOpen(false);
                    }}
                    onDoubleClick={() => startRename(t.id, t.name)}
                    className={cx(
                      "flex flex-1 items-center gap-2 overflow-hidden rounded-[6px] px-2 py-1.5 text-left text-body",
                      t.id === activeId ? "text-accent" : "text-ink",
                    )}
                  >
                    <Check
                      size={13}
                      className={cx(
                        "shrink-0",
                        t.id === activeId ? "text-accent opacity-100" : "opacity-0",
                      )}
                    />
                    <span className="flex-1 truncate">{t.name}</span>
                    <span
                      className={cx(
                        "tabnum shrink-0 text-caption",
                        t.id === activeId ? "text-accent/80" : "text-subtle",
                      )}
                    >
                      {t.count}
                    </span>
                  </button>
                  <button
                    onClick={() => startRename(t.id, t.name)}
                    title="Rename tray"
                    className="flex h-6 w-6 shrink-0 items-center justify-center rounded-[5px] text-subtle opacity-0 hover:bg-ink/10 hover:text-ink group-hover/tray:opacity-100"
                  >
                    <Pencil size={12} />
                  </button>
                  <button
                    onClick={() => confirmDelete(t.id, t.name, t.count)}
                    title="Delete tray"
                    className="flex h-6 w-6 shrink-0 items-center justify-center rounded-[5px] text-subtle opacity-0 hover:bg-red-500/10 hover:text-red-500 group-hover/tray:opacity-100"
                  >
                    <Trash2 size={12} />
                  </button>
                </>
              )}
            </div>
          ))}
          <div className="my-1 h-px bg-hairline/10" />
          <button
            onClick={() => {
              useTray.getState().createTray();
              setOpen(false);
            }}
            className="flex w-full items-center gap-2 rounded-[6px] px-2 py-1.5 text-body text-ink hover:bg-ink/8"
          >
            <Plus size={13} className="text-subtle" />
            New tray
          </button>
        </div>
      )}
    </div>
  );
}
