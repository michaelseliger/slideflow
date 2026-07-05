import { useSlidePreview } from "../lib/useSlideSvg";
import { cx } from "../lib/utils";

interface ThumbnailProps {
  slideId: number;
  alt?: string;
  className?: string;
  /** Render immediately (viewport) vs lazily. */
  enabled?: boolean;
  rounded?: boolean;
}

/**
 * A 16:9 slide thumbnail. Shows a shimmering skeleton until the SVG hydrates,
 * matting the slide on a neutral card so white slides don't glare (esp. dark
 * mode). Text/snippets in the parent render before this image decodes.
 */
export default function Thumbnail({
  slideId,
  alt,
  className,
  enabled = true,
  rounded = true,
}: ThumbnailProps) {
  const { src: uri } = useSlidePreview(slideId, "thumb", enabled);
  return (
    <div
      className={cx(
        "relative w-full overflow-hidden bg-white dark:bg-[#0f0f10]",
        rounded && "rounded-[6px]",
        className,
      )}
      style={{ aspectRatio: "16 / 9" }}
    >
      {uri ? (
        <img
          src={uri}
          alt={alt ?? "Slide preview"}
          draggable={false}
          loading="lazy"
          decoding="async"
          className="absolute inset-0 h-full w-full object-contain"
        />
      ) : (
        <div className="shimmer absolute inset-0" aria-hidden />
      )}
    </div>
  );
}
