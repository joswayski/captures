import { describe, expect, it } from "vitest";

import {
  dragSelectionRect,
  frontToBackWindows,
  isCapturableSelection,
  selectionRect,
} from "./selection";

describe("frontToBackWindows", () => {
  it("uses native z-order instead of the incoming array order", () => {
    expect(frontToBackWindows([
      { id: "rear", z_order: 2 },
      { id: "front", z_order: 20 },
      { id: "middle", z_order: 10 },
    ]).map(({ id }) => id)).toEqual(["front", "middle", "rear"]);
  });

  it("keeps native list order when two windows share a level", () => {
    expect(frontToBackWindows([
      { id: "first", z_order: 4 },
      { id: "second", z_order: 4 },
    ]).map(({ id }) => id)).toEqual(["first", "second"]);
  });
});

describe("selectionRect", () => {
  it("normalizes a reverse drag", () => {
    expect(selectionRect({ x: 300, y: 200 }, { x: 100, y: 50 })).toEqual({
      x: 100,
      y: 50,
      width: 200,
      height: 150,
    });
  });

  it("allows zero-sized previews while the pointer is down", () => {
    expect(selectionRect({ x: 2, y: 3 }, { x: 2, y: 3 })).toEqual({
      x: 2,
      y: 3,
      width: 0,
      height: 0,
    });
  });
});

describe("isCapturableSelection", () => {
  it("rejects a click and one-dimensional drags", () => {
    expect(isCapturableSelection({ x: 10, y: 10, width: 0, height: 0 })).toBe(false);
    expect(isCapturableSelection({ x: 10, y: 10, width: 20, height: 1 })).toBe(false);
  });

  it("accepts a dragged region", () => {
    expect(isCapturableSelection({ x: 10, y: 10, width: 20, height: 30 })).toBe(true);
  });
});

describe("dragSelectionRect", () => {
  const initial = { x: 100, y: 80, width: 400, height: 240 };
  const bounds = { width: 800, height: 600 };

  it("moves a region without allowing it outside the display", () => {
    expect(dragSelectionRect("move", { x: 120, y: 100 }, { x: 900, y: 700 }, initial, bounds)).toEqual({
      x: 400,
      y: 360,
      width: 400,
      height: 240,
    });
  });

  it("resizes from each corner with a usable minimum size", () => {
    expect(dragSelectionRect("nw", { x: 100, y: 80 }, { x: 490, y: 310 }, initial, bounds)).toEqual({
      x: 484,
      y: 304,
      width: 16,
      height: 16,
    });
    expect(dragSelectionRect("se", { x: 500, y: 320 }, { x: 900, y: 700 }, initial, bounds)).toEqual({
      x: 100,
      y: 80,
      width: 700,
      height: 520,
    });
  });
});
