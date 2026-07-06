import { useRef } from "react";
import { useDismiss } from "../lib/useDismiss";

/** Compact reference for the search box's advanced syntax. Purely informational
 *  — the engine owns all parsing; this popover never touches the query. */
export default function SearchHelpPopover({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);

  useDismiss(ref, onClose);

  return (
    <div
      ref={ref}
      className="absolute left-0 top-full z-50 mt-1.5 w-[400px] rounded-[10px] border border-hairline/10 bg-elevated p-4 text-body shadow-peek"
      onClick={(e) => e.stopPropagation()}
    >
      <div className="mb-3 text-body font-semibold text-ink">Search syntax</div>
      <ul className="space-y-2.5">
        <Row token="title:roadmap">match the slide title</Row>
        <Row token={`deck:"Q3"`}>restrict to a deck</Row>
        <Row token="notes:budget">search speaker notes</Row>
        <Row token={`"exact phrase"`}>match a whole phrase</Row>
        <Row token="revenue OR arr">either term</Row>
        <Row token="-draft">exclude a term</Row>
        <Row token="after:2026-01">modified-date range</Row>
      </ul>
      <p className="mt-3 border-t border-hairline/10 pt-2.5 text-caption text-subtle">
        AND by default · prefix matching · accent-insensitive
      </p>
    </div>
  );
}

function Row({ token, children }: { token: string; children: React.ReactNode }) {
  return (
    <li className="flex items-baseline gap-3">
      <code className="shrink-0 whitespace-nowrap rounded bg-ink/[0.06] px-1.5 py-0.5 font-mono text-[12px] leading-tight text-ink">
        {token}
      </code>
      <span className="text-caption text-subtle">{children}</span>
    </li>
  );
}
