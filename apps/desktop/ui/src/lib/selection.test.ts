import { describe, expect, it } from "vitest";

import { selectionRect } from "./selection";

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

