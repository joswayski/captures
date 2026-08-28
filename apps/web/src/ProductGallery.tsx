import { useCallback, useId, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";

import captureControls from "../../../docs/images/capture-controls.jpg";
import captureSelection from "../../../docs/images/capture-selection.jpg";
import preferences from "../../../docs/images/preferences.jpg";
import screenshotEditor from "../../../docs/images/screenshot-editor.jpg";
import videoEditor from "../../../docs/images/video-editor.jpg";
import ProductLightbox from "./ProductLightbox";
import {
  galleryAllowsLightboxOpen,
  galleryAllowsSlideGesture,
  galleryFrameGesture,
} from "./productLightbox";
import { PRODUCT_SHOTS, galleryFrameAspectRatio, type ProductShot } from "./productShots";
import { useImageZoom } from "./useImageZoom";

const SHOT_SRC = {
  "capture-controls.jpg": captureControls,
  "capture-selection.jpg": captureSelection,
  "preferences.jpg": preferences,
  "screenshot-editor.jpg": screenshotEditor,
  "video-editor.jpg": videoEditor,
} as const satisfies Record<ProductShot["file"], string>;

const FRAME_ASPECT = galleryFrameAspectRatio();

export default function ProductGallery() {
  const headingId = useId();
  const captionId = useId();
  const hintId = useId();
  const frameRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const swipeStart = useRef<{ x: number; y: number } | null>(null);
  const stagePointers = useRef(0);
  const [index, setIndex] = useState(0);
  const [lightboxOpen, setLightboxOpen] = useState(false);
  const [openedByPointer, setOpenedByPointer] = useState(false);
  const suppressClick = useRef(false);
  const shot = PRODUCT_SHOTS[index] ?? PRODUCT_SHOTS[0];
  const lastIndex = PRODUCT_SHOTS.length - 1;
  const src = SHOT_SRC[shot.file];
  const imageZoom = useImageZoom({
    active: !lightboxOpen,
    resetKey: shot.file,
    viewportRef: frameRef,
    imageRef,
    wheel: "modified",
  });

  const goTo = useCallback((next: number) => {
    setIndex((next + PRODUCT_SHOTS.length) % PRODUCT_SHOTS.length);
  }, []);

  const previous = useCallback(() => goTo(index - 1), [goTo, index]);
  const next = useCallback(() => goTo(index + 1), [goTo, index]);
  const closeLightbox = useCallback(() => setLightboxOpen(false), []);
  const openLightbox = useCallback((fromPointer: boolean) => {
    setOpenedByPointer(fromPointer);
    setLightboxOpen(true);
  }, []);

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (lightboxOpen) return;
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
    } else if (event.key === "Enter") {
      event.preventDefault();
      openLightbox(false);
    }
  }

  function handleFramePointerDown(event: PointerEvent<HTMLDivElement>) {
    imageZoom.onPointerDown(event);
  }

  function handleFramePointerMove(event: PointerEvent<HTMLDivElement>) {
    imageZoom.onPointerMove(event);
  }

  function handleFramePointerUp(event: PointerEvent<HTMLDivElement>) {
    const release = imageZoom.onPointerUp(event);
    if (!release) return;
    if (!galleryAllowsLightboxOpen(release.scale, release.pinched)) {
      suppressClick.current = true;
    }
  }

  function handleFramePointerCancel() {
    imageZoom.onPointerCancel();
  }

  function handleStagePointerDown(event: PointerEvent<HTMLDivElement>) {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    stagePointers.current += 1;
    if (stagePointers.current === 1) {
      swipeStart.current = { x: event.clientX, y: event.clientY };
    } else {
      swipeStart.current = null;
    }
  }

  function handleStagePointerUp(event: PointerEvent<HTMLDivElement>) {
    stagePointers.current = Math.max(0, stagePointers.current - 1);
    const start = swipeStart.current;
    swipeStart.current = null;
    if (stagePointers.current > 0 || start === null || suppressClick.current) return;
    if (!galleryAllowsSlideGesture(imageZoom.zoom.scale)) return;
    const action = galleryFrameGesture(event.clientX - start.x, event.clientY - start.y);
    if (action === "previous") {
      suppressClick.current = true;
      previous();
    } else if (action === "next") {
      suppressClick.current = true;
      next();
    } else if (action === "ignore") {
      suppressClick.current = true;
    }
  }

  function handleStagePointerCancel() {
    stagePointers.current = 0;
    swipeStart.current = null;
  }

  function handleFrameClick() {
    if (suppressClick.current) {
      suppressClick.current = false;
      return;
    }
    if (imageZoom.zoomed) return;
    openLightbox(true);
  }

  const frameClass = imageZoom.zoomed ? "product-gallery-frame is-zoomed" : "product-gallery-frame";
  const imageClass = imageZoom.settling ? "product-gallery-image is-settling" : "product-gallery-image";

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
        aria-describedby={`${captionId} ${hintId}`}
        tabIndex={0}
        onKeyDown={handleKeyDown}
      >
        <div
          className="product-gallery-stage"
          onPointerDown={handleStagePointerDown}
          onPointerUp={handleStagePointerUp}
          onPointerCancel={handleStagePointerCancel}
        >
          <div
            ref={frameRef}
            className={frameClass}
            style={{ aspectRatio: `1 / ${FRAME_ASPECT}` }}
            aria-haspopup="dialog"
            aria-expanded={lightboxOpen}
            onPointerDown={handleFramePointerDown}
            onPointerMove={handleFramePointerMove}
            onPointerUp={handleFramePointerUp}
            onPointerCancel={handleFramePointerCancel}
            onClick={handleFrameClick}
          >
            <img
              ref={imageRef}
              className={imageClass}
              src={src}
              alt={shot.alt}
              width={shot.width}
              height={shot.height}
              decoding="async"
              draggable={false}
              style={{
                transform: `translate(${imageZoom.zoom.x}px, ${imageZoom.zoom.y}px) scale(${imageZoom.zoom.scale})`,
              }}
            />
          </div>

          <div className="product-gallery-copy">
            <div className="product-gallery-captions">
              {PRODUCT_SHOTS.map((item) => {
                const active = item.id === shot.id;
                return (
                  <p
                    key={item.id}
                    id={active ? captionId : undefined}
                    className="product-gallery-caption"
                    aria-hidden={active ? undefined : "true"}
                  >
                    <span className="font-medium text-ink">{item.title}</span>
                    <span className="text-ink-soft"> · </span>
                    {item.description}
                  </p>
                );
              })}
            </div>
            <p className="product-gallery-index" aria-hidden="true">
              {index + 1}/{PRODUCT_SHOTS.length}
            </p>
          </div>
        </div>
        <p id={hintId} className="sr-only">
          Pinch or use a trackpad pinch to zoom this screenshot. Swipe or use Previous and Next to
          change shots. Tap to open a larger view.
        </p>
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

      <ProductLightbox
        open={lightboxOpen}
        src={src}
        shot={shot}
        onClose={closeLightbox}
        suppressFocusRing={openedByPointer}
      />
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
