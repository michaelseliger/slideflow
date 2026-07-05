import { useEffect, useRef } from "react";

/** Compact reference for the search box's advanced syntax. Purely informational
 *  — the engine owns all parsing; this popover never touches the query. Styled
 *  to match {@link FilterPopover}. */
export default function SearchHelpPopover({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [onClose]);

  return (
    <div
      ref={ref}
      className="absolute left-0 top-full z-50 mt-1.5 w-80 rounded-[8px] border border-hairline/10 bg-elevated p-3 text-body shadow-peek"
      onClick={(e) => e.stopPropagation()}
    >
      <div className="mb-2 text-caption font-semibold uppercase tracking-wide text-subtle/70">
        Search syntax
      </div>
      <ul className="space-y-2">
        <Row tokens={["title:", "deck:", "notes:", "body:"]}>
          Limit a term to one field
        </Row>
        <Row tokens={["“exact phrase”"]}>Match words together, in order</Row>
        <Row tokens={["term OR term"]}>Match either term</Row>
        <Row tokens={["-term", "NOT term"]}>Exclude a term</Row>
        <Row tokens={["after:2026-01-31", "before:2026-01-31"]}>
          Filter by modified date
        </Row>
      </ul>
      <p className="mt-2.5 border-t border-hairline/10 pt-2 text-caption text-subtle/80">
        Terms combine with AND by default. Example:{" "}
        <code className="rounded bg-ink/8 px-1 py-0.5 font-mono text-[11px] text-ink">
          title:roadmap -draft after:2026-01-01
        </code>
      </p>
    </div>
  );
}

function Row({ tokens, children }: { tokens: string[]; children: React.ReactNode }) {
  return (
    <li className="flex items-baseline gap-2">
      <span className="flex shrink-0 flex-wrap gap-1">
        {tokens.map((t) => (
          <code
            key={t}
            className="rounded bg-ink/8 px-1.5 py-0.5 font-mono text-[11px] leading-tight text-ink"
          >
            {t}
          </code>
        ))}
      </span>
      <span className="text-caption text-subtle">{children}</span>
    </li>
  );
}
