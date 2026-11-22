import { useCallback, useEffect, useId, useRef, useState, type MouseEvent } from "react";

import captureSelection from "../../../docs/images/capture-selection.jpg";
import collapsedMiniPreviews from "../../../docs/images/collapsed-mini-previews.jpg";
import preferences from "../../../docs/images/preferences.jpg";
import screenshotEditor from "../../../docs/images/screenshot-editor.jpg";
import videoEditor from "../../../docs/images/video-editor.jpg";
import ProductLightbox from "./ProductLightbox";
import { PRODUCT_SHOTS, type ProductShot } from "./productShots";

const SHOT_SRC = {
  "capture-selection.jpg": captureSelection,
  "collapsed-mini-previews.jpg": collapsedMiniPreviews,
  "preferences.jpg": preferences,
  "screenshot-editor.jpg": screenshotEditor,
  "video-editor.jpg": videoEditor,
} as const satisfies Record<ProductShot["file"], string>;

/**
 * App Store-style strip: every still keeps its own shape at one shared height,
 * the next one peeks in from the edge, and native scroll snapping handles
 * swipes, trackpads, and keyboard scrolling. Clicking a shot opens the lightbox.
 */
export default function ProductGallery() {
  const headingId = useId();
  const trackRef = useRef<HTMLDivElement>(null);
  const [atStart, setAtStart] = useState(true);
  const [atEnd, setAtEnd] = useState(false);
  const [openIndex, setOpenIndex] = useState<number | null>(null);
  const [openedByPointer, setOpenedByPointer] = useState(false);

  const updateEdges = useCallback(() => {
    const track = trackRef.current;
    if (!track) return;
    setAtStart(track.scrollLeft <= 2);
    setAtEnd(track.scrollLeft + track.clientWidth >= track.scrollWidth - 2);
  }, []);

  useEffect(() => {
    updateEdges();
    window.addEventListener("resize", updateEdges);
    return () => window.removeEventListener("resize", updateEdges);
  }, [updateEdges]);

  function step(direction: 1 | -1) {
    const track = trackRef.current;
    if (!track) return;
    const slides = [...track.children] as HTMLElement[];
    const start = slides[0]?.offsetLeft ?? 0;
    const current = track.scrollLeft + start;
    // First slide whose left edge is past (or before) the snapped one.
    const target =
      direction === 1
        ? slides.find((slide) => slide.offsetLeft > current + 2)
        : [...slides].reverse().find((slide) => slide.offsetLeft < current - 2);
    const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    track.scrollTo({
      left: target ? target.offsetLeft - start : direction === 1 ? track.scrollWidth : 0,
      behavior: reduceMotion ? "auto" : "smooth",
    });
  }

  function open(index: number, event: MouseEvent) {
    setOpenedByPointer(event.detail > 0);
    setOpenIndex(index);
  }

  const openShot = openIndex === null ? PRODUCT_SHOTS[0] : PRODUCT_SHOTS[openIndex];

  return (
    <section aria-labelledby={headingId} className="mt-16">
      <div className="flex items-center justify-between gap-4">
        <h2
          id={headingId}
          className="text-[0.6875rem] font-medium uppercase tracking-[0.14em] text-accent-readable"
        >
          A look at Captures
        </h2>
        <div className="flex gap-2">
          <button
            type="button"
            className="gallery-arrow"
            aria-label="Previous screenshot"
            disabled={atStart}
            onClick={() => step(-1)}
          >
            <ChevronIcon direction="left" />
          </button>
          <button
            type="button"
            className="gallery-arrow"
            aria-label="Next screenshot"
            disabled={atEnd}
            onClick={() => step(1)}
          >
            <ChevronIcon direction="right" />
          </button>
        </div>
      </div>

      <div
        ref={trackRef}
        className="product-gallery-track"
        role="region"
        aria-roledescription="carousel"
        aria-label="Product screenshots"
        tabIndex={0}
        onScroll={updateEdges}
      >
        {PRODUCT_SHOTS.map((shot, index) => (
          <figure key={shot.id} className="product-gallery-slide">
            <button
              type="button"
              className="product-gallery-shot"
              aria-haspopup="dialog"
              aria-label={`Open larger view: ${shot.title}`}
              onClick={(event) => open(index, event)}
            >
              <img
                src={SHOT_SRC[shot.file]}
                alt={shot.alt}
                width={shot.width}
                height={shot.height}
                decoding="async"
                loading={index < 2 ? "eager" : "lazy"}
                draggable={false}
              />
            </button>
            <figcaption className="product-gallery-caption">
              <span className="block font-medium text-ink">{shot.title}</span>
              {shot.description}
            </figcaption>
          </figure>
        ))}
      </div>

      <ProductLightbox
        open={openIndex !== null}
        src={SHOT_SRC[openShot.file]}
        shot={openShot}
        onClose={() => setOpenIndex(null)}
        suppressFocusRing={openedByPointer}
      />
    </section>
  );
}

function ChevronIcon({ direction }: { direction: "left" | "right" }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width="14"
      height="14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {direction === "left" ? <path d="M10 3 5 8l5 5" /> : <path d="m6 3 5 5-5 5" />}
    </svg>
  );
}
