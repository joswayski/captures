export const MIN_SCALE = 1;
export const MAX_SCALE = 4;
export const ZOOMED_SCALE = 1.05;
export const DOUBLE_TAP_SCALE = 2.5;
export const ZOOM_BUTTON_FACTOR = 1.4;
export const SWIPE_THRESHOLD_PX = 48;
export const TAP_SLOP_PX = 12;
export const DOUBLE_TAP_MS = 320;
export const CLOSE_SWIPE_PX = 80;

export type Point = { x: number; y: number };
export type Size = { width: number; height: number };
export type ZoomTransform = { scale: number; x: number; y: number };

export const FIT_TRANSFORM: ZoomTransform = { scale: 1, x: 0, y: 0 };

export function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value)) + 0;
}

export function clampScale(scale: number) {
  return clamp(scale, MIN_SCALE, MAX_SCALE);
}

export function isZoomed(scale: number) {
  return scale > ZOOMED_SCALE;
}

export function galleryFrameGesture(
  deltaX: number,
  deltaY: number,
  swipeThreshold = SWIPE_THRESHOLD_PX,
  tapSlop = TAP_SLOP_PX,
): "previous" | "next" | "open" | "ignore" {
  if (Math.abs(deltaX) >= swipeThreshold && Math.abs(deltaX) >= Math.abs(deltaY)) {
    return deltaX > 0 ? "previous" : "next";
  }
  if (Math.hypot(deltaX, deltaY) <= tapSlop) return "open";
  return "ignore";
}

export function galleryAllowsSlideGesture(scale: number) {
  return !isZoomed(scale);
}

export function clampGalleryIndex(index: number, count: number) {
  if (count <= 0) return 0;
  return Math.min(Math.max(index, 0), count - 1);
}

export function galleryHasPrevious(index: number) {
  return index > 0;
}

export function galleryHasNext(index: number, count: number) {
  return count > 0 && index < count - 1;
}

export function galleryAllowsLightboxOpen(scale: number, pinched: boolean) {
  return !pinched && !isZoomed(scale);
}

export function shouldPreventGalleryTouchScroll(touchCount: number, scale: number) {
  return touchCount >= 2 || isZoomed(scale);
}

export function shouldZoomFromWheel(ctrlKey: boolean, metaKey: boolean) {
  return ctrlKey || metaKey;
}

export function clearRestoredDialogFocus(openedByPointer: boolean) {
  if (!openedByPointer) return;
  const active = document.activeElement;
  if (active instanceof HTMLElement && active !== document.body) {
    active.blur();
  }
}

export function pointerDistance(a: Point, b: Point) {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

export function pointerMidpoint(a: Point, b: Point): Point {
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

export function clampPan(
  transform: ZoomTransform,
  viewport: Size,
  fitted: Size,
): ZoomTransform {
  const scaledW = fitted.width * transform.scale;
  const scaledH = fitted.height * transform.scale;
  const maxX = Math.max(0, (scaledW - viewport.width) / 2);
  const maxY = Math.max(0, (scaledH - viewport.height) / 2);
  return {
    scale: transform.scale,
    x: clamp(transform.x, -maxX, maxX),
    y: clamp(transform.y, -maxY, maxY),
  };
}

export function scaleAroundPoint(
  current: ZoomTransform,
  nextScale: number,
  pivot: Point,
  viewport: Size,
  fitted: Size,
): ZoomTransform {
  const scale = clampScale(nextScale);
  const originX = viewport.width / 2;
  const originY = viewport.height / 2;
  const ratio = current.scale === 0 ? 1 : scale / current.scale;
  return clampPan(
    {
      scale,
      x: ratio * current.x + (1 - ratio) * (pivot.x - originX),
      y: ratio * current.y + (1 - ratio) * (pivot.y - originY),
    },
    viewport,
    fitted,
  );
}

export function toggleZoom(
  current: ZoomTransform,
  pivot: Point,
  viewport: Size,
  fitted: Size,
): ZoomTransform {
  if (isZoomed(current.scale)) return FIT_TRANSFORM;
  return scaleAroundPoint(current, DOUBLE_TAP_SCALE, pivot, viewport, fitted);
}

export function zoomFromCenter(
  current: ZoomTransform,
  nextScale: number,
  viewport: Size,
  fitted: Size,
): ZoomTransform {
  return scaleAroundPoint(
    current,
    nextScale,
    { x: viewport.width / 2, y: viewport.height / 2 },
    viewport,
    fitted,
  );
}

export function wheelScaleFactor(deltaY: number) {
  return clamp(Math.exp(-deltaY * 0.0015), 0.5, 2);
}

export function shouldCloseOnSwipe(deltaX: number, deltaY: number, scale: number) {
  return !isZoomed(scale) && deltaY > CLOSE_SWIPE_PX && deltaY > Math.abs(deltaX);
}

export function isDoubleTap(previous: { time: number; x: number; y: number } | null, next: Point, now: number) {
  if (!previous) return false;
  return now - previous.time <= DOUBLE_TAP_MS && pointerDistance(previous, next) <= TAP_SLOP_PX * 2;
}
