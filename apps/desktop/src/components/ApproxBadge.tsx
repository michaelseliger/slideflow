import { AlertTriangle } from "lucide-react";

/** Human-readable labels for each dropped-construct kind the renderer reports. */
export const DROP_KIND_LABELS: Record<string, string> = {
  chart: "charts",
  smartart: "SmartArt",
  ole: "embedded objects",
  "unsupported-image": "an unsupported image",
  "unknown-shape": "unsupported shapes",
};

export function dropKindLabel(k: string): string {
  return DROP_KIND_LABELS[k] ?? k;
}

/** "Approximate preview" badge shown when the renderer skipped constructs the
 *  export still keeps. `tile` sits in the persistent indicator stack on a slide
 *  card; `peek` reads inline in the peek-modal header. */
export default function ApproxBadge({
  dropped,
  variant,
}: {
  dropped: string[];
  variant: "tile" | "peek";
}) {
  if (dropped.length === 0) return null;

  const list = dropped.map(dropKindLabel).join(", ");
  const title = `Approximate preview — not everything on this slide is shown here (${list}). The exported slide keeps its original content.`;

  if (variant === "peek") {
    return (
      <span
        title={title}
        className="inline-flex items-center gap-1.5 rounded-full bg-amber-400/15 px-2.5 py-1 text-caption font-medium text-amber-600 dark:text-amber-300"
      >
        <AlertTriangle size={12} />
        Approximate preview
      </span>
    );
  }

  return (
    <div
      title={title}
      className="inline-flex items-center gap-1 rounded-full bg-amber-400 px-1.5 py-0.5 text-[10px] font-medium text-black shadow"
    >
      <AlertTriangle size={10} />
      Approximate
    </div>
  );
}
