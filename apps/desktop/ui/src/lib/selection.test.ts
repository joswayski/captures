import { describe, expect, it } from "vitest";

import {
  captureDimClipPath,
  dragSelectionRect,
  frontToBackWindows,
  isCapturableSelection,
  roundedRectPath,
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

describe("roundedRectPath", () => {
  it("builds a rounded window cutout", () => {
    expect(roundedRectPath(
      { x: 100, y: 120, width: 300, height: 220 },
      10,
    )).toBe(
      "M110 120H390A10 10 0 0 1 400 130V330A10 10 0 0 1 390 340"
      + "H110A10 10 0 0 1 100 330V130A10 10 0 0 1 110 120Z",
    );
  });

  it("keeps region cutouts square and clamps oversized radii", () => {
    expect(roundedRectPath(
      { x: 10, y: 20, width: 30, height: 40 },
      0,
    )).toBe("M10 20H40V60H10Z");
    expect(roundedRectPath(
      { x: 0, y: 0, width: 12, height: 8 },
      20,
    )).toContain("A4 4");
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

describe("captureDimClipPath", () => {
  it("cuts a hole using the same CSS pixel coords as the marquee", () => {
    expect(captureDimClipPath({ x: 100, y: 120, width: 313, height: 415 })).toBe(
      "polygon(evenodd, 0% 0%, 100% 0%, 100% 100%, 0% 100%, 0% 0%, "
      + "100px 120px, 100px 535px, 413px 535px, 413px 120px, 100px 120px)",
    );
  });

  it("clamps negative origins so the hole stays on-screen", () => {
    expect(captureDimClipPath({ x: -10, y: -4, width: 40, height: 20 })).toBe(
      "polygon(evenodd, 0% 0%, 100% 0%, 100% 100%, 0% 100%, 0% 0%, "
      + "0px 0px, 0px 20px, 40px 20px, 40px 0px, 0px 0px)",
    );
  });

  it("closes the hole ring so polygon auto-close cannot fan from the origin", () => {
    // Without the final return to the hole start, CSS polygon() draws an edge
    // from the hole's last corner back to 0% 0% — the top-left "spotlight".
    const path = captureDimClipPath({ x: 40, y: 30, width: 200, height: 120 });
    expect(path.endsWith("40px 30px, 40px 150px, 240px 150px, 240px 30px, 40px 30px)")).toBe(true);
  });
});
