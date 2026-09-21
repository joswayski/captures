import { describe, expect, it } from "vitest";
import cases from "../../../../../crates/captures-app/tests/editor-image-background-cases.json";
import { documentPointToImagePixel, removeColorToTransparent } from "./imageBackground";
import type { EditorImageElement } from "./screenshotEditor";

describe("native image-background vectors agree with the shipping editor", () => {
  it.each(cases.mapping)("maps $orientation at $point with rotation $rotation", (entry) => {
    const image = { ...cases.image, ...entry } as EditorImageElement;
    const point = documentPointToImagePixel(image, entry.point);
    expect(point ? [point.x, point.y] : null).toEqual(entry.expected);
  });

  it.each(cases.wand)("clears exact RGBA samples: $name", (entry) => {
    const image = {
      width: cases.pixels.width,
      height: cases.pixels.height,
      data: new Uint8ClampedArray(cases.pixels.rgba),
    } as ImageData;
    const expected = [...cases.pixels.rgba];
    for (const index of entry.cleared) expected.fill(0, index * 4, index * 4 + 4);
    expect(removeColorToTransparent(image, entry.seed[0], entry.seed[1], entry.tolerance,
      entry.contiguous)).toBe(entry.cleared.length);
    expect([...image.data]).toEqual(expected);
  });
});
