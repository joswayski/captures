import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
} from "react";

import {
  FIT_TRANSFORM,
  MAX_SCALE,
  clampPan,
  isZoomed,
  pointerDistance,
  pointerMidpoint,
  scaleAroundPoint,
  shouldPreventGalleryTouchScroll,
  shouldZoomFromWheel,
  wheelScaleFactor,
  zoomFromCenter,
  type Point,
  type Size,
  type ZoomTransform,
} from "./productLightboxModel";

export type ImageZoomRelease = {
  deltaX: number;
  deltaY: number;
  moved: boolean;
  scale: number;
  pinched: boolean;
  pointerType: string;
};

type UseImageZoomOptions = {
  active: boolean;
  resetKey: string;
  viewportRef: RefObject<HTMLElement | null>;
  imageRef: RefObject<HTMLImageElement | null>;
  wheel?: "always" | "modified" | "never";
};

function touchPoint(touch: Touch): Point {
  return { x: touch.clientX, y: touch.clientY };
}

export function useImageZoom({
  active,
  resetKey,
  viewportRef,
  imageRef,
  wheel = "modified",
}: UseImageZoomOptions) {
  const zoomRef = useRef<ZoomTransform>(FIT_TRANSFORM);
  const fittedRef = useRef<Size>({ width: 0, height: 0 });
  const viewportSizeRef = useRef<Size>({ width: 0, height: 0 });
  const pointersRef = useRef(new Map<number, Point>());
  const pinchRef = useRef<{ distance: number; transform: ZoomTransform } | null>(null);
  const panRef = useRef<{ x: number; y: number; transform: ZoomTransform } | null>(null);
  const gestureStartRef = useRef<{ x: number; y: number } | null>(null);
  const movedRef = useRef(false);
  const pinchedRef = useRef(false);
  const [zoom, setZoom] = useState<ZoomTransform>(FIT_TRANSFORM);
  const [settling, setSettling] = useState(false);

  const commit = useCallback((next: ZoomTransform, animate: boolean) => {
    const clamped = clampPan(next, viewportSizeRef.current, fittedRef.current);
    zoomRef.current = clamped;
    setSettling(animate);
    setZoom(clamped);
  }, []);

  const reset = useCallback((animate = false) => {
    zoomRef.current = FIT_TRANSFORM;
    pinchedRef.current = false;
    pinchRef.current = null;
    panRef.current = null;
    pointersRef.current.clear();
    gestureStartRef.current = null;
    movedRef.current = false;
    setSettling(animate);
    setZoom(FIT_TRANSFORM);
  }, []);

  const identityRef = useRef({ active, resetKey });
  if (identityRef.current.active !== active || identityRef.current.resetKey !== resetKey) {
    identityRef.current = { active, resetKey };
    zoomRef.current = FIT_TRANSFORM;
    pinchedRef.current = false;
    pinchRef.current = null;
    panRef.current = null;
    pointersRef.current.clear();
    gestureStartRef.current = null;
    movedRef.current = false;
    if (zoom.scale !== 1 || zoom.x !== 0 || zoom.y !== 0 || settling) {
      setSettling(false);
      setZoom(FIT_TRANSFORM);
    }
  }

  const zoomBy = useCallback(
    (factor: number) => {
      commit(
        zoomFromCenter(
          zoomRef.current,
          zoomRef.current.scale * factor,
          viewportSizeRef.current,
          fittedRef.current,
        ),
        true,
      );
    },
    [commit],
  );

  const pinchTo = useCallback((first: Point, second: Point) => {
    const viewportEl = viewportRef.current;
    if (viewportEl === null) return;
    if (!pinchRef.current) {
      pinchRef.current = {
        distance: Math.max(1, pointerDistance(first, second)),
        transform: zoomRef.current,
      };
      return;
    }
    const distance = Math.max(1, pointerDistance(first, second));
    const mid = pointerMidpoint(first, second);
    const rect = viewportEl.getBoundingClientRect();
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
  }, [commit, viewportRef]);

  useEffect(() => {
    if (!active) return;
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
  }, [active, resetKey, imageRef, viewportRef]);

  useEffect(() => {
    if (!active || wheel === "never") return;
    const viewport = viewportRef.current;
    if (viewport === null) return;
    const viewportEl = viewport;
    const wheelMode = wheel;

    function onWheel(event: WheelEvent) {
      if (wheelMode === "modified" && !shouldZoomFromWheel(event.ctrlKey, event.metaKey)) return;
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
  }, [active, resetKey, wheel, viewportRef, commit]);

  useEffect(() => {
    if (!active) return;
    const viewport = viewportRef.current;
    if (viewport === null) return;
    const viewportEl = viewport;

    function applyPinch(first: Point, second: Point) {
      pinchTo(first, second);
    }

    function onTouchStart(event: TouchEvent) {
      if (event.touches.length >= 2) {
        pinchedRef.current = true;
        panRef.current = null;
        const first = event.touches[0];
        const second = event.touches[1];
        if (first && second) {
          pinchRef.current = {
            distance: Math.max(1, pointerDistance(touchPoint(first), touchPoint(second))),
            transform: zoomRef.current,
          };
        }
      }
    }

    function onTouchMove(event: TouchEvent) {
      if (shouldPreventGalleryTouchScroll(event.touches.length, zoomRef.current.scale)) {
        event.preventDefault();
      }
      if (event.touches.length < 2) return;
      pinchedRef.current = true;
      const first = event.touches[0];
      const second = event.touches[1];
      if (first && second) applyPinch(touchPoint(first), touchPoint(second));
    }

    function onTouchEnd(event: TouchEvent) {
      if (event.touches.length < 2) pinchRef.current = null;
    }

    function preventSafariPageZoom(event: Event) {
      event.preventDefault();
    }

    viewportEl.addEventListener("touchstart", onTouchStart, { passive: true });
    viewportEl.addEventListener("touchmove", onTouchMove, { passive: false });
    viewportEl.addEventListener("touchend", onTouchEnd);
    viewportEl.addEventListener("touchcancel", onTouchEnd);
    viewportEl.addEventListener("gesturestart", preventSafariPageZoom, { passive: false });
    viewportEl.addEventListener("gesturechange", preventSafariPageZoom, { passive: false });
    return () => {
      viewportEl.removeEventListener("touchstart", onTouchStart);
      viewportEl.removeEventListener("touchmove", onTouchMove);
      viewportEl.removeEventListener("touchend", onTouchEnd);
      viewportEl.removeEventListener("touchcancel", onTouchEnd);
      viewportEl.removeEventListener("gesturestart", preventSafariPageZoom);
      viewportEl.removeEventListener("gesturechange", preventSafariPageZoom);
    };
  }, [active, resetKey, viewportRef, pinchTo]);

  function onPointerDown(event: ReactPointerEvent<HTMLElement>) {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    pointersRef.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    movedRef.current = false;
    if (pointersRef.current.size === 1) {
      gestureStartRef.current = { x: event.clientX, y: event.clientY };
      pinchedRef.current = false;
    }
    if (pointersRef.current.size >= 2) {
      panRef.current = null;
      pinchedRef.current = true;
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

  function onPointerMove(event: ReactPointerEvent<HTMLElement>) {
    const previous = pointersRef.current.get(event.pointerId);
    if (!previous) return;
    if (Math.hypot(event.clientX - previous.x, event.clientY - previous.y) > 6) {
      movedRef.current = true;
    }
    pointersRef.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (pointersRef.current.size >= 2) {
      pinchedRef.current = true;
      const [first, second] = Array.from(pointersRef.current.values());
      if (first && second) pinchTo(first, second);
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

  function onPointerUp(event: ReactPointerEvent<HTMLElement>): ImageZoomRelease | null {
    pointersRef.current.delete(event.pointerId);
    if (pointersRef.current.size < 2) pinchRef.current = null;
    if (pointersRef.current.size > 0) return null;

    const start = gestureStartRef.current;
    const moved = movedRef.current;
    const pinched = pinchedRef.current;
    let scale = zoomRef.current.scale;
    gestureStartRef.current = null;
    panRef.current = null;
    movedRef.current = false;
    pinchedRef.current = false;

    if (!isZoomed(scale) && scale !== 1) {
      commit(FIT_TRANSFORM, true);
      scale = 1;
    }

    if (!start) return null;
    return {
      deltaX: event.clientX - start.x,
      deltaY: event.clientY - start.y,
      moved,
      scale,
      pinched,
      pointerType: event.pointerType,
    };
  }

  function onPointerCancel() {
    pointersRef.current.clear();
    pinchRef.current = null;
    panRef.current = null;
    gestureStartRef.current = null;
    movedRef.current = false;
    pinchedRef.current = false;
  }

  return {
    zoom,
    zoomRef,
    fittedRef,
    viewportSizeRef,
    settling,
    zoomed: isZoomed(zoom.scale),
    atMinZoom: !isZoomed(zoom.scale),
    atMaxZoom: zoom.scale >= MAX_SCALE - 0.01,
    commit,
    reset,
    zoomBy,
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onPointerCancel,
  };
}
