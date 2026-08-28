import assert from "node:assert/strict";
import test from "node:test";

import {
  CLOSE_SWIPE_PX,
  DOUBLE_TAP_SCALE,
  FIT_TRANSFORM,
  MAX_SCALE,
  MIN_SCALE,
  SWIPE_THRESHOLD_PX,
  clampPan,
  clampScale,
  galleryFrameGesture,
  isDoubleTap,
  pointerDistance,
  pointerMidpoint,
  scaleAroundPoint,
  shouldCloseOnSwipe,
  toggleZoom,
  wheelScaleFactor,
  zoomFromCenter,
} from "./productLightbox.ts";

const viewport = { width: 400, height: 800 };
const fitted = { width: 360, height: 240 };

test("gallery taps open the lightbox and swipes change slides", () => {
  assert.equal(galleryFrameGesture(0, 0), "open");
  assert.equal(galleryFrameGesture(8, -4), "open");
  assert.equal(galleryFrameGesture(SWIPE_THRESHOLD_PX, 0), "previous");
  assert.equal(galleryFrameGesture(120, 20), "previous");
  assert.equal(galleryFrameGesture(-SWIPE_THRESHOLD_PX, 8), "next");
  assert.equal(galleryFrameGesture(20, 80), "ignore");
});

test("pinch helpers measure span and midpoint", () => {
  assert.equal(pointerDistance({ x: 0, y: 0 }, { x: 3, y: 4 }), 5);
  assert.deepEqual(pointerMidpoint({ x: 0, y: 10 }, { x: 10, y: 0 }), { x: 5, y: 5 });
});

test("scale stays within the lightbox range", () => {
  assert.equal(clampScale(0.2), MIN_SCALE);
  assert.equal(clampScale(8), MAX_SCALE);
  assert.equal(clampScale(2), 2);
});

test("pan is locked when the shot still fits, then clamped when zoomed", () => {
  assert.deepEqual(
    clampPan({ scale: 1, x: 80, y: -40 }, viewport, fitted),
    FIT_TRANSFORM,
  );

  const overflow = { width: 400, height: 700 };
  const zoomed = clampPan({ scale: 3, x: 2000, y: -2000 }, viewport, overflow);
  assert.equal(zoomed.scale, 3);
  assert.equal(zoomed.x, (overflow.width * 3 - viewport.width) / 2);
  assert.equal(zoomed.y, -((overflow.height * 3 - viewport.height) / 2));
});

test("pinch zoom keeps the pivot under the fingers", () => {
  const wide = { width: 400, height: 700 };
  const pivot = { x: 80, y: 200 };
  const next = scaleAroundPoint(FIT_TRANSFORM, 2, pivot, viewport, wide);
  assert.equal(next.scale, 2);
  assert.equal(next.x, -(pivot.x - viewport.width / 2));
  assert.equal(next.y, -(pivot.y - viewport.height / 2));
});

test("toggle zoom enlarges from the tap, then returns to fit", () => {
  const pivot = { x: 200, y: 400 };
  const zoomed = toggleZoom(FIT_TRANSFORM, pivot, viewport, fitted);
  assert.equal(zoomed.scale, DOUBLE_TAP_SCALE);
  assert.deepEqual(toggleZoom(zoomed, pivot, viewport, fitted), FIT_TRANSFORM);
});

test("center zoom buttons keep the shot in the middle of the viewport", () => {
  const next = zoomFromCenter(FIT_TRANSFORM, 2, viewport, { width: 400, height: 700 });
  assert.equal(next.scale, 2);
  assert.equal(next.x, 0);
  assert.equal(next.y, 0);
});

test("wheel zoom is exponential and bounded", () => {
  assert.ok(wheelScaleFactor(-100) > 1);
  assert.ok(wheelScaleFactor(100) < 1);
  assert.equal(wheelScaleFactor(-10_000), 2);
  assert.equal(wheelScaleFactor(10_000), 0.5);
});

test("a downward flick closes the lightbox only when not zoomed", () => {
  assert.equal(shouldCloseOnSwipe(0, CLOSE_SWIPE_PX + 1, 1), true);
  assert.equal(shouldCloseOnSwipe(120, CLOSE_SWIPE_PX + 1, 1), false);
  assert.equal(shouldCloseOnSwipe(0, CLOSE_SWIPE_PX + 1, 2), false);
});

test("double-tap detection uses a short window and nearby points", () => {
  const first = { time: 1_000, x: 40, y: 40 };
  assert.equal(isDoubleTap(null, { x: 40, y: 40 }, 1_100), false);
  assert.equal(isDoubleTap(first, { x: 44, y: 42 }, 1_250), true);
  assert.equal(isDoubleTap(first, { x: 44, y: 42 }, 1_500), false);
  assert.equal(isDoubleTap(first, { x: 200, y: 40 }, 1_100), false);
});
