import { useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { cx } from "../lib/utils";
import { useDismiss } from "../lib/useDismiss";

export interface MenuItem {
  label: string;
  onClick: () => void;
  danger?: boolean;
  separatorBefore?: boolean;
  disabled?: boolean;
}

interface ContextMenuProps {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}

/** A native-feeling right-click menu rendered in a portal, auto-flipped to stay
 *  on-screen and dismissed on outside click / Escape / scroll. */
export default function ContextMenu({ x, y, items, onClose }: ContextMenuProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    let nx = x;
    let ny = y;
    if (x + rect.width > window.innerWidth - 8) nx = window.innerWidth - rect.width - 8;
    if (y + rect.height > window.innerHeight - 8) ny = window.innerHeight - rect.height - 8;
    setPos({ x: Math.max(8, nx), y: Math.max(8, ny) });
  }, [x, y]);

  useDismiss(ref, onClose, { onScroll: true });

  return createPortal(
    <div
      ref={ref}
      role="menu"
      className="fixed z-[100] min-w-[220px] rounded-[8px] border border-hairline/10 bg-elevated/95 p-1 text-body shadow-peek material"
      style={{ left: pos.x, top: pos.y }}
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((item, i) => (
        <div key={i}>
          {item.separatorBefore && (
            <div className="my-1 h-px bg-hairline/10" role="separator" />
          )}
          <button
            role="menuitem"
            disabled={item.disabled}
            className={cx(
              "flex w-full items-center rounded-[5px] px-2.5 py-1.5 text-left",
              item.disabled
                ? "cursor-default text-subtle/50"
                : "hover:bg-accent hover:text-white",
              item.danger && !item.disabled && "text-red-500 hover:bg-red-500",
            )}
            onClick={() => {
              if (item.disabled) return;
              item.onClick();
              onClose();
            }}
          >
            {item.label}
          </button>
        </div>
      ))}
    </div>,
    document.body,
  );
}
