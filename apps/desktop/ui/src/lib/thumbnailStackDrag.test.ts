import { afterEach, describe, expect, it } from "vitest";

import {
  CollapsedThumbnailStackDrag,
  THUMBNAIL_HARNESS_DRAG_X_VAR,
  THUMBNAIL_HARNESS_DRAG_Y_VAR,
  THUMBNAIL_STACK_DRAG_THRESHOLD_PX,
  THUMBNAIL_STACK_DRAGGING_CLASS,
  applyThumbnailStackDragSway,
  clampThumbnailStackFrame,
  clearThumbnailStackDragSway,
  parseCssPx,
  preventThumbnailHtml5Drag,
  readHarnessStackOffset,
  setThumbnailStackDragging,
  thumbnailStackDragExceededThreshold,
  thumbnailStackDragSway,
  writeHarnessStackOffset,
} from "./thumbnailStackDrag";

afterEach(() => {
  document.documentElement.style.removeProperty(THUMBNAIL_HARNESS_DRAG_X_VAR);
  document.documentElement.style.removeProperty(THUMBNAIL_HARNESS_DRAG_Y_VAR);
  document.body.replaceChildren();
});

describe("preventThumbnailHtml5Drag", () => {
  it("cancels the browser drag so pointer-drag can keep the pile", () => {
    const event = new Event("dragstart", { bubbles: true, cancelable: true });
    preventThumbnailHtml5Drag(event);
    expect(event.defaultPrevented).toBe(true);
  });
});

describe("thumbnailStackDragExceededThreshold", () => {
  it("ignores sub-threshold jitter so a click can still expand", () => {
    expect(thumbnailStackDragExceededThreshold(3, 4)).toBe(false);
    expect(thumbnailStackDragExceededThreshold(THUMBNAIL_STACK_DRAG_THRESHOLD_PX, 0)).toBe(true);
  });
});

describe("thumbnailStackDragSway", () => {
  it("lags opposite the drag so rear cards trail the pointer", () => {
    expect(thumbnailStackDragSway(40, 0)).toEqual({ x: -12, y: 0 });
    expect(thumbnailStackDragSway(-20, 30).x).toBeGreaterThan(0);
    expect(thumbnailStackDragSway(0, 40).y).toBeLessThan(0);
  });

  it("skips sway when motion is reduced", () => {
    expect(thumbnailStackDragSway(40, 20, { reducedMotion: true })).toEqual({ x: 0, y: 0 });
  });
});

describe("clampThumbnailStackFrame", () => {
  const work = { x: 0, y: 0, width: 1_920, height: 1_040, bottomGap: 12 };

  it("keeps the frame inside the work area with the system-chrome gap", () => {
    expect(clampThumbnailStackFrame(500, 400, 340, 240, work)).toEqual({ x: 500, y: 400 });
    expect(clampThumbnailStackFrame(-40, -20, 340, 240, work)).toEqual({ x: 0, y: 0 });
    expect(clampThumbnailStackFrame(2_000, 2_000, 340, 240, work)).toEqual({
      x: 1_580,
      y: 788,
    });
  });
});

describe("harness stack offset", () => {
  it("parses CSS pixel variables and clamps the mock pile to the viewport", () => {
    expect(parseCssPx("")).toBe(0);
    expect(parseCssPx("18.5px")).toBe(18.5);

    const next = writeHarnessStackOffset(120, -80, document.documentElement, {
      width: 1_280,
      height: 720,
    });
    expect(next).toEqual({ x: 120, y: -80 });
    expect(readHarnessStackOffset()).toEqual({ x: 120, y: -80 });

    expect(writeHarnessStackOffset(4_000, 80, document.documentElement, {
      width: 1_280,
      height: 720,
    })).toEqual({ x: 940, y: 0 });
  });
});

describe("CollapsedThumbnailStackDrag", () => {
  it("expands when the pointer is released without crossing the drag threshold", async () => {
    const frames: { x: number; y: number }[] = [];
    const drag = new CollapsedThumbnailStackDrag({
      getFrame: () => ({ x: 0, y: 0 }),
      moveFrame: (x, y) => {
        frames.push({ x, y });
        return { x, y };
      },
      reducedMotion: () => false,
    });

    expect(drag.pointerDown({ button: 0, pointerId: 1, screenX: 10, screenY: 20 })).toBe(true);
    expect(await drag.pointerMove({ pointerId: 1, screenX: 14, screenY: 22 })).toEqual({
      dragging: false,
      x: 0,
      y: 0,
      sway: { x: 0, y: 0 },
    });
    expect(await drag.pointerUp({ pointerId: 1 })).toBe("expand");
    expect(frames).toEqual([]);
  });

  it("moves from the press origin once the threshold is crossed", async () => {
    const drag = new CollapsedThumbnailStackDrag({
      getFrame: () => ({ x: 40, y: 80 }),
      moveFrame: (x, y) => ({ x, y }),
      reducedMotion: () => false,
    });

    drag.pointerDown({ button: 0, pointerId: 7, screenX: 100, screenY: 200 });
    const moved = await drag.pointerMove({ pointerId: 7, screenX: 160, screenY: 188 });
    expect(moved).toEqual({
      dragging: true,
      x: 100,
      y: 68,
      sway: thumbnailStackDragSway(60, -12),
    });
    expect(drag.isDragging).toBe(true);
    expect(await drag.pointerUp({ pointerId: 7 })).toBe("drop");
    expect(drag.isDragging).toBe(false);
  });

  it("ignores a second button and a mismatched pointer id", async () => {
    const drag = new CollapsedThumbnailStackDrag({
      getFrame: () => ({ x: 0, y: 0 }),
      moveFrame: (x, y) => ({ x, y }),
      reducedMotion: () => false,
    });

    expect(drag.pointerDown({ button: 2, pointerId: 1, screenX: 0, screenY: 0 })).toBe(false);
    drag.pointerDown({ button: 0, pointerId: 1, screenX: 0, screenY: 0 });
    expect(await drag.pointerMove({ pointerId: 2, screenX: 40, screenY: 0 })).toBeNull();
    expect(await drag.pointerUp({ pointerId: 2 })).toBe("ignored");
  });
});

describe("stack dragging class helpers", () => {
  it("toggles the dragging class and clears sway on release", () => {
    const stack = document.createElement("main");
    applyThumbnailStackDragSway(stack, { x: 6, y: -2 });
    setThumbnailStackDragging(stack, true);
    expect(stack).toHaveClass(THUMBNAIL_STACK_DRAGGING_CLASS);
    expect(stack.style.getPropertyValue("--thumbnail-drag-sway-x")).toBe("6");

    setThumbnailStackDragging(stack, false);
    expect(stack).not.toHaveClass(THUMBNAIL_STACK_DRAGGING_CLASS);
    expect(stack.style.getPropertyValue("--thumbnail-drag-sway-x")).toBe("0");
    clearThumbnailStackDragSway(stack);
  });
});
