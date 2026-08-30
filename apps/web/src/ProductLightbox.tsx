import { useEffect, useId, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";

import type { ProductShot } from "./productShots";
import {
  FIT_TRANSFORM,
  MAX_SCALE,
  ZOOM_BUTTON_FACTOR,
  clampPan,
  clearRestoredDialogFocus,
  isZoomed,
  pointerDistance,
  pointerMidpoint,
  scaleAroundPoint,
  shouldCloseOnSwipe,
  toggleZoom,
  wheelScaleFactor,
  zoomFromCenter,
  type Point,
  type Size,
  type ZoomTransform,
} from "./productLightboxModel";

type ProductLightboxProps = {
  open: boolean;
  src: string;
  shot: ProductShot;
  onClose: () => void;
  suppressFocusRing?: boolean;
};

export default function ProductLightbox({
  open,
  src,
  shot,
  onClose,
  suppressFocusRing = false,
}: ProductLightboxProps) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const zoomRef = useRef<ZoomTransform>(FIT_TRANSFORM);
  const fittedRef = useRef<Size>({ width: 0, height: 0 });
  const viewportSizeRef = useRef<Size>({ width: 0, height: 0 });
  const pointersRef = useRef(new Map<number, Point>());
  const pinchRef = useRef<{ distance: number; transform: ZoomTransform } | null>(null);
  const panRef = useRef<{ x: number; y: number; transform: ZoomTransform } | null>(null);
  const gestureStartRef = useRef<{ x: number; y: number; onImage: boolean } | null>(null);
  const movedRef = useRef(false);
  const [zoom, setZoom] = useState<ZoomTransform>(FIT_TRANSFORM);
  const [settling, setSettling] = useState(false);

  function commit(next: ZoomTransform, animate: boolean) {
    const clamped = clampPan(next, viewportSizeRef.current, fittedRef.current);
    zoomRef.current = clamped;
    setSettling(animate);
    setZoom(clamped);
  }

  function zoomBy(factor: number) {
    commit(
      zoomFromCenter(
        zoomRef.current,
        zoomRef.current.scale * factor,
        viewportSizeRef.current,
        fittedRef.current,
      ),
      true,
    );
  }

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (open) {
      if (!dialog.open) dialog.showModal();
      zoomRef.current = FIT_TRANSFORM;
      setZoom(FIT_TRANSFORM);
      setSettling(false);
    } else if (dialog.open) {
      dialog.close();
      // Closing a modal restores focus as if it were keyboard navigation, which
      // leaves a blue :focus-visible ring on the gallery after a tap.
      clearRestoredDialogFocus(suppressFocusRing);
      requestAnimationFrame(() => {
        requestAnimationFrame(() => clearRestoredDialogFocus(suppressFocusRing));
      });
    }
  }, [open, suppressFocusRing]);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    function handleClose() {
      onClose();
    }
    dialog.addEventListener("close", handleClose);
    return () => dialog.removeEventListener("close", handleClose);
  }, [onClose]);

  useEffect(() => {
    if (!open) return;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = previousOverflow;
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const image = imageRef.current;
    const viewport = viewportRef.current;
    if (image === null || viewport === null) return;
    const imageEl = image;
    const viewportEl = viewport;

    function updateSize() {
      fittedRef.current = { width: imageEl.offsetWidth, height: imageEl.offsetHeight };
      viewportSizeRef.current = { width: viewportEl.clientWidth, height: viewportEl.clientHeight };
      const clamped = clampPan(zoomRef.current, viewportSizeRef.current, fittedRef.current);
      zoomRef.current = clamped;
      setZoom(clamped);
    }

    const observer = new ResizeObserver(updateSize);
    observer.observe(imageEl);
    observer.observe(viewportEl);
    imageEl.addEventListener("load", updateSize);
    updateSize();
    return () => {
      observer.disconnect();
      imageEl.removeEventListener("load", updateSize);
    };
  }, [open, src]);

  useEffect(() => {
    if (!open) return;
    const viewport = viewportRef.current;
    if (viewport === null) return;
    const viewportEl = viewport;

    function onWheel(event: WheelEvent) {
      event.preventDefault();
      const rect = viewportEl.getBoundingClientRect();
      commit(
        scaleAroundPoint(
          zoomRef.current,
          zoomRef.current.scale * wheelScaleFactor(event.deltaY),
          { x: event.clientX - rect.left, y: event.clientY - rect.top },
          viewportSizeRef.current,
          fittedRef.current,
        ),
        false,
      );
    }

    viewportEl.addEventListener("wheel", onWheel, { passive: false });
    return () => viewportEl.removeEventListener("wheel", onWheel);
  }, [open, src]);

  function handlePointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    pointersRef.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    movedRef.current = false;
    const onImage = event.target === imageRef.current;
    if (pointersRef.current.size === 1) {
      gestureStartRef.current = { x: event.clientX, y: event.clientY, onImage };
    }
    if (pointersRef.current.size === 2) {
      panRef.current = null;
      const [first, second] = Array.from(pointersRef.current.values());
      if (first && second) {
        pinchRef.current = {
          distance: Math.max(1, pointerDistance(first, second)),
          transform: zoomRef.current,
        };
      }
      return;
    }
    if (isZoomed(zoomRef.current.scale)) {
      panRef.current = { x: event.clientX, y: event.clientY, transform: zoomRef.current };
    }
  }

  function handlePointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    const previous = pointersRef.current.get(event.pointerId);
    if (!previous) return;
    if (Math.hypot(event.clientX - previous.x, event.clientY - previous.y) > 6) {
      movedRef.current = true;
    }
    pointersRef.current.set(event.pointerId, { x: event.clientX, y: event.clientY });

    if (pointersRef.current.size >= 2 && pinchRef.current && viewportRef.current) {
      const [first, second] = Array.from(pointersRef.current.values());
      if (!first || !second) return;
      const distance = Math.max(1, pointerDistance(first, second));
      const mid = pointerMidpoint(first, second);
      const rect = viewportRef.current.getBoundingClientRect();
      commit(
        scaleAroundPoint(
          pinchRef.current.transform,
          pinchRef.current.transform.scale * (distance / pinchRef.current.distance),
          { x: mid.x - rect.left, y: mid.y - rect.top },
          viewportSizeRef.current,
          fittedRef.current,
        ),
        false,
      );
      return;
    }

    if (panRef.current && isZoomed(zoomRef.current.scale)) {
      commit(
        {
          scale: panRef.current.transform.scale,
          x: panRef.current.transform.x + event.clientX - panRef.current.x,
          y: panRef.current.transform.y + event.clientY - panRef.current.y,
        },
        false,
      );
    }
  }

  function handlePointerUp(event: ReactPointerEvent<HTMLDivElement>) {
    pointersRef.current.delete(event.pointerId);
    if (pointersRef.current.size < 2) pinchRef.current = null;
    if (pointersRef.current.size > 0) return;

    const start = gestureStartRef.current;
    const moved = movedRef.current;
    const onImage = start?.onImage ?? event.target === imageRef.current;
    gestureStartRef.current = null;
    panRef.current = null;
    movedRef.current = false;

    if (!start) return;
    const deltaX = event.clientX - start.x;
    const deltaY = event.clientY - start.y;
    if (shouldCloseOnSwipe(deltaX, deltaY, zoomRef.current.scale)) {
      onClose();
      return;
    }
    if (moved || !onImage) return;

    commit(
      toggleZoom(
        zoomRef.current,
        viewportPoint(event.clientX, event.clientY),
        viewportSizeRef.current,
        fittedRef.current,
      ),
      true,
    );
  }

  function handlePointerCancel() {
    pointersRef.current.clear();
    pinchRef.current = null;
    panRef.current = null;
    gestureStartRef.current = null;
    movedRef.current = false;
  }

  function viewportPoint(clientX: number, clientY: number): Point {
    const rect = viewportRef.current?.getBoundingClientRect();
    if (!rect) return { x: clientX, y: clientY };
    return { x: clientX - rect.left, y: clientY - rect.top };
  }

  const zoomed = isZoomed(zoom.scale);
  const atMinZoom = !zoomed;
  const atMaxZoom = zoom.scale >= MAX_SCALE - 0.01;

  return (
    <dialog ref={dialogRef} className="product-lightbox" aria-labelledby={titleId}>
      <h2 id={titleId} className="sr-only">
        {shot.title}
      </h2>
      <div className="product-lightbox-toolbar">
        <div className="product-lightbox-zoom-cluster">
          <button
            type="button"
            className="product-lightbox-tool"
            aria-label="Zoom out"
            disabled={atMinZoom}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={() => zoomBy(1 / ZOOM_BUTTON_FACTOR)}
          >
            <MinusIcon />
          </button>
          <button
            type="button"
            className="product-lightbox-tool"
            aria-label="Zoom in"
            disabled={atMaxZoom}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={() => zoomBy(ZOOM_BUTTON_FACTOR)}
          >
            <PlusIcon />
          </button>
        </div>
        <button
          type="button"
          className="product-lightbox-tool"
          onPointerDown={(event) => event.stopPropagation()}
          onClick={onClose}
        >
          <CloseIcon />
          <span className="sr-only">Close</span>
        </button>
      </div>
      <div
        ref={viewportRef}
        className={zoomed ? "product-lightbox-viewport is-zoomed" : "product-lightbox-viewport"}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={handlePointerCancel}
      >
        <img
          ref={imageRef}
          className={settling ? "product-lightbox-image is-settling" : "product-lightbox-image"}
          src={src}
          alt={shot.alt}
          width={shot.width}
          height={shot.height}
          draggable={false}
          style={{ transform: `translate(${zoom.x}px, ${zoom.y}px) scale(${zoom.scale})` }}
        />
      </div>
      <p className="product-lightbox-hint">
        {shot.title}
        <span aria-hidden="true"> · </span>
        {zoomed ? "Drag to pan" : "Tap image or + to zoom"}
      </p>
    </dialog>
  );
}

function CloseIcon() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="14"
      height="14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <path d="M4 4l8 8M12 4l-8 8" />
    </svg>
  );
}

function PlusIcon() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="14"
      height="14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <path d="M8 3.5v9M3.5 8h9" />
    </svg>
  );
}

function MinusIcon() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="14"
      height="14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <path d="M3.5 8h9" />
    </svg>
  );
}
