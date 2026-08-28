import { useCallback, useId, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";

import captureControls from "../../../docs/images/capture-controls.jpg";
import captureSelection from "../../../docs/images/capture-selection.jpg";
import preferences from "../../../docs/images/preferences.jpg";
import screenshotEditor from "../../../docs/images/screenshot-editor.jpg";
import videoEditor from "../../../docs/images/video-editor.jpg";
import { PRODUCT_SHOTS, type ProductShot } from "./productShots";

const SHOT_SRC = {
  "capture-controls.jpg": captureControls,
  "capture-selection.jpg": captureSelection,
  "preferences.jpg": preferences,
  "screenshot-editor.jpg": screenshotEditor,
  "video-editor.jpg": videoEditor,
} as const satisfies Record<ProductShot["file"], string>;

const SWIPE_THRESHOLD_PX = 48;

export default function ProductGallery() {
  const headingId = useId();
  const captionId = useId();
  const [index, setIndex] = useState(0);
  const swipeStartX = useRef<number | null>(null);
  const shot = PRODUCT_SHOTS[index] ?? PRODUCT_SHOTS[0];
  const lastIndex = PRODUCT_SHOTS.length - 1;

  const goTo = useCallback((next: number) => {
    setIndex((next + PRODUCT_SHOTS.length) % PRODUCT_SHOTS.length);
  }, []);

  const previous = useCallback(() => goTo(index - 1), [goTo, index]);
  const next = useCallback(() => goTo(index + 1), [goTo, index]);

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      previous();
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      next();
    } else if (event.key === "Home") {
      event.preventDefault();
      goTo(0);
    } else if (event.key === "End") {
      event.preventDefault();
      goTo(lastIndex);
    }
  }

  function handlePointerDown(event: PointerEvent<HTMLDivElement>) {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    swipeStartX.current = event.clientX;
  }

  function handlePointerUp(event: PointerEvent<HTMLDivElement>) {
    const startX = swipeStartX.current;
    swipeStartX.current = null;
    if (startX === null) return;
    const delta = event.clientX - startX;
    if (delta > SWIPE_THRESHOLD_PX) previous();
    else if (delta < -SWIPE_THRESHOLD_PX) next();
  }

  function handlePointerCancel() {
    swipeStartX.current = null;
  }

  return (
    <section aria-labelledby={headingId} className="mt-14 border-t border-border pt-10">
      <h2
        id={headingId}
        className="text-[0.6875rem] font-medium uppercase tracking-[0.14em] text-accent-readable"
      >
        A look at Captures
      </h2>

      <div
        className="product-gallery mt-6"
        role="region"
        aria-roledescription="carousel"
        aria-label="Product screenshots"
        aria-describedby={captionId}
        tabIndex={0}
        onKeyDown={handleKeyDown}
      >
        <div
          className="product-gallery-frame"
          onPointerDown={handlePointerDown}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerCancel}
        >
          <img
            key={shot.file}
            className="product-gallery-image"
            src={SHOT_SRC[shot.file]}
            alt={shot.alt}
            width={shot.width}
            height={shot.height}
            decoding="async"
            draggable={false}
          />
        </div>

        <div className="mt-4 flex items-start justify-between gap-3">
          <p id={captionId} className="min-w-0 text-sm leading-relaxed text-ink-muted">
            <span className="font-medium text-ink">{shot.title}</span>
            <span className="text-ink-soft"> · </span>
            {shot.description}
          </p>
          <p className="shrink-0 pt-0.5 text-[0.625rem] tabular-nums text-ink-soft" aria-hidden="true">
            {index + 1}/{PRODUCT_SHOTS.length}
          </p>
        </div>
        <p className="sr-only" aria-live="polite">
          Screenshot {index + 1} of {PRODUCT_SHOTS.length}: {shot.title}
        </p>

        <div className="mt-4 flex items-center justify-between gap-3">
          <button type="button" className="gallery-nav" onClick={previous}>
            <ChevronIcon direction="left" />
            Previous
          </button>
          <div className="flex items-center gap-1.5" role="group" aria-label="Choose screenshot">
            {PRODUCT_SHOTS.map((item, itemIndex) => {
              const selected = itemIndex === index;
              return (
                <button
                  key={item.id}
                  type="button"
                  className="gallery-dot"
                  aria-label={item.title}
                  aria-current={selected ? "true" : undefined}
                  onClick={() => goTo(itemIndex)}
                />
              );
            })}
          </div>
          <button type="button" className="gallery-nav" onClick={next}>
            Next
            <ChevronIcon direction="right" />
          </button>
        </div>
      </div>
    </section>
  );
}

function ChevronIcon({ direction }: { direction: "left" | "right" }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width="12"
      height="12"
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
