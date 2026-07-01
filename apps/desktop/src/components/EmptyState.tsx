import { motion } from "framer-motion";
import { useApp } from "../stores/useApp";
import { prefersReducedMotion } from "../lib/utils";

/** First-run onboarding: one friendly illustration, one sentence, one button.
 *  No wizard, no carousel, no consent wall. */
export default function EmptyState() {
  const addFolder = useApp((s) => s.addFolder);
  const reduce = prefersReducedMotion();

  return (
    <div className="flex h-full flex-col items-center justify-center bg-canvas px-8 text-center">
      <motion.div
        initial={reduce ? false : { scale: 0.9, opacity: 0, y: 8 }}
        animate={{ scale: 1, opacity: 1, y: 0 }}
        transition={{ type: "spring", stiffness: 260, damping: 22 }}
      >
        <SlidesIllustration />
      </motion.div>

      <h1 className="mt-7 max-w-md text-heading font-semibold text-ink">
        Point Slideflow at the folders where your decks live
      </h1>
      <p className="mt-2 max-w-sm text-body text-subtle">
        We'll index every slide so you can search them instantly — and compose
        new decks that keep each slide's original formatting.
      </p>

      <button
        onClick={() => void addFolder()}
        className="mt-6 rounded-[8px] bg-accent px-5 py-2.5 text-body font-semibold text-white shadow-tile transition-transform hover:scale-[1.02] active:scale-95"
      >
        Choose a folder…
      </button>
    </div>
  );
}

/** A restrained, professional hand-drawn-feel stack of slides. */
function SlidesIllustration() {
  return (
    <svg width="184" height="140" viewBox="0 0 184 140" fill="none" aria-hidden>
      <defs>
        <linearGradient id="sf-a" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="rgb(var(--accent-rgb))" stopOpacity="0.16" />
          <stop offset="1" stopColor="rgb(var(--accent-rgb))" stopOpacity="0.04" />
        </linearGradient>
      </defs>
      <rect
        x="34"
        y="20"
        width="116"
        height="70"
        rx="8"
        transform="rotate(-6 92 55)"
        fill="rgb(var(--surface-rgb))"
        stroke="rgb(var(--hairline-rgb) / 0.12)"
      />
      <rect
        x="30"
        y="34"
        width="124"
        height="76"
        rx="8"
        fill="url(#sf-a)"
        stroke="rgb(var(--accent-rgb) / 0.5)"
        strokeWidth="1.5"
      />
      <rect x="44" y="48" width="46" height="7" rx="3.5" fill="rgb(var(--accent-rgb) / 0.8)" />
      <rect x="44" y="62" width="96" height="5" rx="2.5" fill="rgb(var(--subtle-rgb) / 0.5)" />
      <rect x="44" y="72" width="80" height="5" rx="2.5" fill="rgb(var(--subtle-rgb) / 0.4)" />
      <rect x="44" y="82" width="88" height="5" rx="2.5" fill="rgb(var(--subtle-rgb) / 0.3)" />
      <circle cx="150" cy="104" r="18" fill="rgb(var(--accent-rgb))" />
      <path
        d="M150 96v16M142 104h16"
        stroke="#fff"
        strokeWidth="2.5"
        strokeLinecap="round"
      />
    </svg>
  );
}
