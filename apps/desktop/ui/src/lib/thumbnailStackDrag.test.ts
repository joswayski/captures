import { afterEach, describe, expect, it } from "vitest";

import {
  CollapsedThumbnailStackDrag,
  THUMBNAIL_HARNESS_DRAG_X_VAR,
  THUMBNAIL_HARNESS_DRAG_Y_VAR,
  THUMBNAIL_STACK_DRAG_THRESHOLD_PX,
  THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX,
  THUMBNAIL_STACK_DRAG_SWAY_CLASS,
  THUMBNAIL_STACK_DRAGGING_CLASS,
  THUMBNAIL_STACK_PRESSING_CLASS,
  applyThumbnailStackDragSway,
  clampThumbnailStackFrame,
  clearThumbnailStackDragSway,
  parseCssPx,
  cssUrl,
  preventThumbnailHtml5Drag,
  readHarnessStackOffset,
  setThumbnailStackDragSwayReady,
  setThumbnailStackDragging,
  setThumbnailStackPressing,
  thumbnailStackDragExceededThreshold,
  tickThumbnailStackDragSway,
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

describe("cssUrl", () => {
  it("quotes data URLs so they are valid CSS url() values", () => {
    expect(cssUrl('data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg">'))
      .toBe('url("data:image/svg+xml,<svg xmlns=\\"http://www.w3.org/2000/svg\\">")');
  });
});

describe("thumbnailStackDragExceededThreshold", () => {
  it("ignores sub-threshold jitter so a click can still expand", () => {
    expect(thumbnailStackDragExceededThreshold(3, 4)).toBe(false);
    expect(thumbnailStackDragExceededThreshold(THUMBNAIL_STACK_DRAG_THRESHOLD_PX, 0)).toBe(true);
  });
});

describe("tickThumbnailStackDragSway", () => {
  const rest = { x: 0, y: 0 };

  it("lags opposite the latest step so rear cards trail the hands", () => {
    const right = tickThumbnailStackDragSway(rest, { dx: 24, dy: 0, dtMs: 16 });
    expect(right.x).toBeLessThan(0);
    expect(right.x).toBeGreaterThanOrEqual(-THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX);

    const left = tickThumbnailStackDragSway(rest, { dx: -20, dy: 0, dtMs: 16 });
    expect(left.x).toBeGreaterThan(0);

    const down = tickThumbnailStackDragSway(rest, { dx: 0, dy: 20, dtMs: 16 });
    expect(down.y).toBeLessThan(0);
  });

  it("tracks velocity instead of the press origin", () => {
    const right = tickThumbnailStackDragSway(rest, { dx: 8, dy: 0, dtMs: 16 });
    const stillRight = tickThumbnailStackDragSway(right, { dx: 8, dy: 0, dtMs: 16 });
    expect(right.x).toBeLessThan(0);
    expect(stillRight.x).toBeLessThan(right.x);

    const reversing = tickThumbnailStackDragSway(right, { dx: -8, dy: 0, dtMs: 16 });
    expect(reversing.x).toBeGreaterThan(right.x);

    const settling = tickThumbnailStackDragSway(right, { dx: 0, dy: 0, dtMs: 16 });
    expect(Math.abs(settling.x)).toBeLessThan(Math.abs(right.x));
  });

  it("clamps extreme flicks and skips sway when motion is reduced", () => {
    expect(tickThumbnailStackDragSway(rest, { dx: 400, dy: 0, dtMs: 16 }).x)
      .toBe(-THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX);
    expect(tickThumbnailStackDragSway(rest, { dx: 24, dy: 12, dtMs: 16 }, { reducedMotion: true }))
      .toEqual(rest);
  });

  it("keeps a typical pointer step far below the clamp so rear cards stay close", () => {
    const right = tickThumbnailStackDragSway(rest, { dx: 24, dy: 0, dtMs: 16 });
    expect(Math.abs(right.x)).toBeLessThan(THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX);
    expect(Math.abs(right.x)).toBeLessThan(5);
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

  it("lets a bottom-aligned pile reach the top when the window is taller", () => {
    // Preserved expanded frame (4 cards) with the collapsed pile at the bottom.
    expect(clampThumbnailStackFrame(0, -800, 340, 792, work, 240)).toEqual({
      x: 0,
      y: -552,
    });
    expect(clampThumbnailStackFrame(0, 400, 340, 792, work, 240)).toEqual({
      x: 0,
      y: 236,
    });
    expect(clampThumbnailStackFrame(40, 800, 340, 792, work, 240, "top")).toEqual({
      x: 40,
      y: 788,
    });
    expect(clampThumbnailStackFrame(40, -20, 340, 792, work, 240, "top")).toEqual({
      x: 40,
      y: 0,
    });
  });

  it("lets a top-aligned pile reach the bottom when the window is taller", () => {
    expect(clampThumbnailStackFrame(0, -800, 340, 792, work, 240, "top")).toEqual({
      x: 0,
      y: 0,
    });
    expect(clampThumbnailStackFrame(0, 2_000, 340, 792, work, 240, "top")).toEqual({
      x: 0,
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

    const top = writeHarnessStackOffset(40, 80, document.documentElement, {
      width: 1_280,
      height: 720,
    }, { anchor: "top", contentHeight: 240 });
    expect(top).toEqual({ x: 40, y: 80 });
    expect(writeHarnessStackOffset(40, 800, document.documentElement, {
      width: 1_280,
      height: 720,
    }, { anchor: "top", contentHeight: 240 })).toEqual({ x: 40, y: 480 });
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
    expect(moved?.dragging).toBe(true);
    expect(moved?.x).toBe(100);
    expect(moved?.y).toBe(68);
    expect(moved?.sway.x).toBeLessThan(0);
    expect(moved?.sway.y).toBeGreaterThan(0);
    expect(drag.isDragging).toBe(true);
    expect(await drag.pointerUp({ pointerId: 7 })).toBe("drop");
    expect(drag.isDragging).toBe(false);
  });

  it("starts a later drag from the frame after the previous drop", async () => {
    let frame = { x: 40, y: 80 };
    const drag = new CollapsedThumbnailStackDrag({
      getFrame: () => frame,
      moveFrame: (x, y) => {
        frame = { x, y };
        return frame;
      },
      reducedMotion: () => false,
    });

    drag.pointerDown({ button: 0, pointerId: 1, screenX: 100, screenY: 200 });
    await drag.pointerMove({ pointerId: 1, screenX: 160, screenY: 188 });
    expect(await drag.pointerUp({ pointerId: 1 })).toBe("drop");
    expect(frame).toEqual({ x: 100, y: 68 });

    drag.pointerDown({ button: 0, pointerId: 1, screenX: 160, screenY: 188 });
    const moved = await drag.pointerMove({ pointerId: 1, screenX: 180, screenY: 208 });
    expect(moved?.dragging).toBe(true);
    expect(moved?.x).toBe(120);
    expect(moved?.y).toBe(88);
    expect(await drag.pointerUp({ pointerId: 1 })).toBe("drop");
  });

  it("ignores an in-flight move after the press has already ended", async () => {
    let continueMove: (() => void) | undefined;
    const drag = new CollapsedThumbnailStackDrag({
      getFrame: () => ({ x: 0, y: 0 }),
      moveFrame: (x, y) => new Promise<{ x: number; y: number }>((resolve) => {
        continueMove = () => resolve({ x, y });
      }),
      reducedMotion: () => false,
    });

    drag.pointerDown({ button: 0, pointerId: 1, screenX: 0, screenY: 0 });
    const move = drag.pointerMove({ pointerId: 1, screenX: 40, screenY: 0 });
    await Promise.resolve();
    expect(await drag.pointerUp({ pointerId: 1 })).toBe("drop");
    continueMove?.();
    expect(await move).toBeNull();
    expect(drag.isDragging).toBe(false);
    expect(drag.isActive).toBe(false);
  });

  it("follows later pointer steps instead of freezing the first lean", async () => {
    let now = 0;
    const drag = new CollapsedThumbnailStackDrag({
      getFrame: () => ({ x: 0, y: 0 }),
      moveFrame: (x, y) => ({ x, y }),
      reducedMotion: () => false,
      now: () => now,
    });

    drag.pointerDown({ button: 0, pointerId: 1, screenX: 0, screenY: 0 });
    now = 16;
    const right = await drag.pointerMove({ pointerId: 1, screenX: 12, screenY: 0 });
    now = 32;
    const stillRight = await drag.pointerMove({ pointerId: 1, screenX: 24, screenY: 0 });
    now = 48;
    const left = await drag.pointerMove({ pointerId: 1, screenX: 12, screenY: 0 });

    expect(right?.sway.x).toBeLessThan(0);
    expect(stillRight?.sway.x).toBeLessThan(right!.sway.x);
    expect(left?.sway.x).toBeGreaterThan(stillRight!.sway.x);
  });

  it("marks dragging before the frame moves so the lean can start immediately", async () => {
    const events: string[] = [];
    const drag = new CollapsedThumbnailStackDrag({
      getFrame: () => ({ x: 0, y: 0 }),
      moveFrame: (x, y) => {
        events.push("move");
        return { x, y };
      },
      reducedMotion: () => false,
      onDraggingChange: () => {
        events.push("drag");
      },
    });

    drag.pointerDown({ button: 0, pointerId: 1, screenX: 0, screenY: 0 });
    await drag.pointerMove({ pointerId: 1, screenX: 20, screenY: 0 });
    expect(events[0]).toBe("drag");
    expect(events[1]).toBe("move");
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

  it("clears accumulated lean so gather can hand off from rest", async () => {
    const sways: { x: number; y: number }[] = [];
    const drag = new CollapsedThumbnailStackDrag({
      getFrame: () => ({ x: 0, y: 0 }),
      moveFrame: (x, y) => ({ x, y }),
      reducedMotion: () => false,
      onSway: (sway) => {
        sways.push({ ...sway });
      },
    });

    drag.pointerDown({ button: 0, pointerId: 1, screenX: 0, screenY: 0 });
    await drag.pointerMove({ pointerId: 1, screenX: 20, screenY: 0 });
    expect(sways.at(-1)?.x).toBeLessThan(0);
    drag.resetSway();
    expect(sways.at(-1)).toEqual({ x: 0, y: 0 });
  });

  it("rebases the press origin after a top/bottom coordinate switch", async () => {
    const drag = new CollapsedThumbnailStackDrag({
      getFrame: () => ({ x: 0, y: 0 }),
      moveFrame: (x, y) => ({ x, y }),
      reducedMotion: () => false,
    });

    drag.pointerDown({ button: 0, pointerId: 1, screenX: 10, screenY: 20 });
    await drag.pointerMove({ pointerId: 1, screenX: 30, screenY: 50 });
    drag.rebaseFrame({ x: 100, y: -200 });
    const moved = await drag.pointerMove({ pointerId: 1, screenX: 40, screenY: 60 });
    expect(moved?.x).toBe(110);
    expect(moved?.y).toBe(-190);
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
    expect(stack).not.toHaveClass(THUMBNAIL_STACK_DRAG_SWAY_CLASS);
    expect(stack.style.getPropertyValue("--thumbnail-drag-sway-x")).toBe("0");
    clearThumbnailStackDragSway(stack);
  });

  it("toggles the pressing class so hover fan stays down during a press", () => {
    const stack = document.createElement("main");
    setThumbnailStackPressing(stack, true);
    expect(stack).toHaveClass(THUMBNAIL_STACK_PRESSING_CLASS);
    setThumbnailStackPressing(stack, false);
    expect(stack).not.toHaveClass(THUMBNAIL_STACK_PRESSING_CLASS);
  });

  it("holds drag sway until the hover fan has gathered", () => {
    const stack = document.createElement("main");
    setThumbnailStackDragging(stack, true);
    expect(stack).toHaveClass(THUMBNAIL_STACK_DRAGGING_CLASS);
    expect(stack).not.toHaveClass(THUMBNAIL_STACK_DRAG_SWAY_CLASS);
    setThumbnailStackDragSwayReady(stack, true);
    expect(stack).toHaveClass(THUMBNAIL_STACK_DRAG_SWAY_CLASS);
    setThumbnailStackDragging(stack, false);
    expect(stack).not.toHaveClass(THUMBNAIL_STACK_DRAG_SWAY_CLASS);
  });
});
