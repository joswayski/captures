import { describe, expect, it } from "vitest";

import {
  captureDimClipPath,
  constrainSelectionToAspect,
  dragSelectionRect,
  effectiveDragAspectRatio,
  frontmostCaptureTargetAtPoint,
  frontmostWindowAtPoint,
  frontToBackWindows,
  keepReadyWindowTargets,
  windowListingIsReady,
  windowPointerHoverAtPoint,
  isCapturableSelection,
  parseAspectRatioPreset,
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

describe("frontmostWindowAtPoint", () => {
  const origin = { x: 0, y: 0 };
  const front = { id: "front", z_order: 20, x: 0, y: 0, width: 200, height: 200 };
  const rear = { id: "rear", z_order: 10, x: 50, y: 50, width: 200, height: 200 };

  it("returns the frontmost overlapping window, not the first in the array", () => {
    expect(frontmostWindowAtPoint([rear, front], { x: 100, y: 100 }, origin)?.id).toBe("front");
  });

  it("falls through to a rear window when the pointer is outside the front frame", () => {
    expect(frontmostWindowAtPoint([rear, front], { x: 220, y: 220 }, origin)?.id).toBe("rear");
  });

  it("converts Quartz window bounds into overlay space", () => {
    expect(frontmostWindowAtPoint(
      [{ id: "prefs", z_order: 5, x: 100, y: 80, width: 800, height: 600 }],
      { x: 50, y: 40 },
      { x: 50, y: 40 },
      1,
    )?.id).toBe("prefs");
  });

  it("uses half-open edges so a shared boundary belongs to the next window", () => {
    const left = { id: "left", z_order: 2, x: 0, y: 0, width: 100, height: 100 };
    const right = { id: "right", z_order: 1, x: 100, y: 0, width: 100, height: 100 };
    expect(frontmostWindowAtPoint([left, right], { x: 100, y: 10 }, origin)?.id).toBe("right");
    expect(frontmostWindowAtPoint([left, right], { x: 99.9, y: 10 }, origin)?.id).toBe("left");
  });

  it("returns null when the pointer is not over any window", () => {
    expect(frontmostWindowAtPoint([front, rear], { x: 400, y: 400 }, origin)).toBeNull();
  });
});

describe("frontmostCaptureTargetAtPoint", () => {
  const origin = { x: 0, y: 0 };
  const maximized = { id: "app", z_order: 10, x: 0, y: 0, width: 1440, height: 900 };
  const menuBar = { id: "menubar", z_order: 50, x: 0, y: 0, width: 1440, height: 24 };

  it("lets shell chrome win over a maximized window underneath", () => {
    expect(frontmostCaptureTargetAtPoint(
      [maximized],
      [menuBar],
      { x: 20, y: 8 },
      origin,
    )).toMatchObject({ kind: "chrome", target: { id: "menubar" } });
  });

  it("still selects the app when the pointer is below the chrome strip", () => {
    expect(frontmostCaptureTargetAtPoint(
      [maximized],
      [menuBar],
      { x: 20, y: 80 },
      origin,
    )).toMatchObject({ kind: "window", target: { id: "app" } });
  });
});

describe("windowPointerHoverAtPoint", () => {
  const origin = { x: 0, y: 0 };
  const maximized = { id: "app", z_order: 10, x: 0, y: 0, width: 1440, height: 900 };
  const menuBar = { id: "menubar", z_order: 50, x: 0, y: 0, width: 1440, height: 24 };

  it("hovers the frontmost window under the pointer", () => {
    expect(windowPointerHoverAtPoint(
      [maximized],
      [menuBar],
      { x: 20, y: 80 },
      origin,
      1,
      true,
    )).toEqual({ windowId: "app", display: false });
  });

  it("treats menu-bar chrome as the display instead of the app behind it", () => {
    expect(windowPointerHoverAtPoint(
      [maximized],
      [menuBar],
      { x: 20, y: 8 },
      origin,
      1,
      true,
    )).toEqual({ windowId: null, display: true });
  });

  it("falls back to the display when listing is ready and nothing is hit", () => {
    expect(windowPointerHoverAtPoint(
      [maximized],
      [menuBar],
      { x: 2000, y: 8 },
      origin,
      1,
      true,
    )).toEqual({ windowId: null, display: true });
  });

  it("stays clear while window listing is still deferred", () => {
    expect(windowPointerHoverAtPoint(
      [],
      [],
      { x: 20, y: 80 },
      origin,
      1,
      false,
    )).toEqual({ windowId: null, display: false });
  });
});

describe("windowListingIsReady", () => {
  it("treats a missing flag as ready so older payloads still commit misses", () => {
    expect(windowListingIsReady(undefined)).toBe(true);
    expect(windowListingIsReady(true)).toBe(true);
    expect(windowListingIsReady(false)).toBe(false);
  });
});

describe("keepReadyWindowTargets", () => {
  const ready = {
    id: "selection-1",
    snapshot_url: "capture://recording-selection/selection-1",
    display: { id: "display-1" },
    windows: [{ id: "front-window" }],
    shell_chrome: [{ id: "menubar" }],
    windows_ready: true,
  };

  it("keeps listed windows when a later wake repeats the deferred empty payload", () => {
    expect(keepReadyWindowTargets(ready, {
      ...ready,
      windows: [],
      shell_chrome: [],
      windows_ready: false,
    })).toEqual(ready);
  });

  it("accepts a finished listing, including an empty desktop", () => {
    const emptyReady = {
      ...ready,
      windows: [],
      shell_chrome: [],
      windows_ready: true,
    };
    expect(keepReadyWindowTargets(ready, emptyReady)).toEqual(emptyReady);
  });

  it("does not keep windows after the freeze-frame display changes", () => {
    const incoming = {
      ...ready,
      display: { id: "display-2" },
      windows: [],
      windows_ready: false,
    };
    expect(keepReadyWindowTargets(ready, incoming)).toEqual(incoming);
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

describe("parseAspectRatioPreset", () => {
  it("parses common ratios and treats free as null", () => {
    expect(parseAspectRatioPreset("free")).toBeNull();
    expect(parseAspectRatioPreset("1:1")).toBe(1);
    expect(parseAspectRatioPreset("16:9")).toBeCloseTo(16 / 9);
    expect(parseAspectRatioPreset("9:16")).toBeCloseTo(9 / 16);
    expect(parseAspectRatioPreset("nope")).toBeNull();
  });
});

describe("effectiveDragAspectRatio", () => {
  it("lets Shift force a square over the selector ratio", () => {
    expect(effectiveDragAspectRatio(16 / 9, true)).toBe(1);
    expect(effectiveDragAspectRatio(null, true)).toBe(1);
    expect(effectiveDragAspectRatio(16 / 9, false)).toBeCloseTo(16 / 9);
    expect(effectiveDragAspectRatio(null, false)).toBeNull();
  });
});

describe("constrainSelectionToAspect", () => {
  const bounds = { width: 1440, height: 900 };

  it("leaves freeform selections unchanged", () => {
    const rect = { x: 100, y: 80, width: 320, height: 200 };
    expect(constrainSelectionToAspect(rect, null, bounds)).toEqual(rect);
  });

  it("snaps a wide 16:9 region to a centered 1:1 square inside the box", () => {
    // 320×180 is 16:9. Inscribed 1:1 keeps height 180, shrinks width to 180, centers X.
    const rect = constrainSelectionToAspect(
      { x: 100, y: 50, width: 320, height: 180 },
      1,
      bounds,
    );
    expect(rect.width).toBeCloseTo(180);
    expect(rect.height).toBeCloseTo(180);
    expect(rect.x).toBeCloseTo(100 + (320 - 180) / 2);
    expect(rect.y).toBeCloseTo(50);
    expect(rect.width / rect.height).toBeCloseTo(1);
  });

  it("snaps a square to 16:9 while staying inside the original box", () => {
    const rect = constrainSelectionToAspect(
      { x: 200, y: 100, width: 400, height: 400 },
      16 / 9,
      bounds,
    );
    expect(rect.width / rect.height).toBeCloseTo(16 / 9, 5);
    expect(rect.width).toBeCloseTo(400);
    expect(rect.height).toBeCloseTo(400 * 9 / 16);
    expect(rect.x).toBeCloseTo(200);
    expect(rect.y).toBeCloseTo(100 + (400 - rect.height) / 2);
  });

  it("never expands past the original selection bounds", () => {
    const original = { x: 40, y: 60, width: 160, height: 90 };
    const rect = constrainSelectionToAspect(original, 9 / 16, bounds);
    expect(rect.x).toBeGreaterThanOrEqual(original.x - 1e-6);
    expect(rect.y).toBeGreaterThanOrEqual(original.y - 1e-6);
    expect(rect.x + rect.width).toBeLessThanOrEqual(original.x + original.width + 1e-6);
    expect(rect.y + rect.height).toBeLessThanOrEqual(original.y + original.height + 1e-6);
    expect(rect.width / rect.height).toBeCloseTo(9 / 16, 5);
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

  it("creates a freeform region from a reverse drag", () => {
    expect(dragSelectionRect(
      "create",
      { x: 300, y: 200 },
      { x: 100, y: 50 },
      { x: 300, y: 200, width: 0, height: 0 },
      bounds,
    )).toEqual({ x: 100, y: 50, width: 200, height: 150 });
  });

  it("creates a square when Shift forces 1:1", () => {
    const rect = dragSelectionRect(
      "create",
      { x: 100, y: 100 },
      { x: 300, y: 180 },
      { x: 100, y: 100, width: 0, height: 0 },
      bounds,
      { forceSquare: true },
    );
    expect(rect.width).toBeCloseTo(rect.height);
    expect(rect.width).toBeCloseTo(200);
    expect(rect.x).toBe(100);
    expect(rect.y).toBe(100);
  });

  it("creates with a fixed 16:9 aspect from the selector", () => {
    const rect = dragSelectionRect(
      "create",
      { x: 50, y: 50 },
      { x: 370, y: 400 },
      { x: 50, y: 50, width: 0, height: 0 },
      bounds,
      { aspectRatio: 16 / 9 },
    );
    expect(rect.width / rect.height).toBeCloseTo(16 / 9, 5);
    expect(rect.x).toBe(50);
    expect(rect.y).toBe(50);
  });

  it("lets Shift override a selected 16:9 create drag to a square", () => {
    const rect = dragSelectionRect(
      "create",
      { x: 50, y: 50 },
      { x: 370, y: 400 },
      { x: 50, y: 50, width: 0, height: 0 },
      bounds,
      { aspectRatio: 16 / 9, forceSquare: true },
    );
    expect(rect.width).toBeCloseTo(rect.height);
  });

  it("resizes a corner with a locked aspect from the free corner", () => {
    // SE free corner; opposite NW stays fixed at (100, 80). Pointer at (700, 500)
    // proposes 600×420 (taller than 16:9), so width grows to match height, then
    // clamps to remaining display width from the anchor (700).
    const rect = dragSelectionRect(
      "se",
      { x: 500, y: 320 },
      { x: 700, y: 500 },
      initial,
      bounds,
      { aspectRatio: 16 / 9 },
    );
    expect(rect.x).toBe(100);
    expect(rect.y).toBe(80);
    expect(rect.width / rect.height).toBeCloseTo(16 / 9, 5);
    expect(rect.width).toBeCloseTo(700);
    expect(rect.height).toBeCloseTo(700 * 9 / 16);
  });

  it("keeps a square on Shift corner resize even when freeform was selected", () => {
    const rect = dragSelectionRect(
      "se",
      { x: 500, y: 320 },
      { x: 700, y: 500 },
      initial,
      bounds,
      { forceSquare: true },
    );
    expect(rect.width).toBeCloseTo(rect.height);
    // Dominant free-axis reach is 600×420 → square side 600, clamped by remaining
    // display height from the NW anchor (600 - 80 = 520).
    expect(rect.width).toBeCloseTo(520);
  });

  it("clamps aspect-locked resize to the display bounds", () => {
    const rect = dragSelectionRect(
      "se",
      { x: 500, y: 320 },
      { x: 2000, y: 2000 },
      initial,
      bounds,
      { aspectRatio: 1 },
    );
    expect(rect.x).toBe(100);
    expect(rect.y).toBe(80);
    expect(rect.width).toBeCloseTo(rect.height);
    expect(rect.x + rect.width).toBeLessThanOrEqual(bounds.width);
    expect(rect.y + rect.height).toBeLessThanOrEqual(bounds.height);
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
