import { describe, expect, it } from "vitest";
import cases from "../../../../../crates/captures-app/tests/editor-image-background-cases.json";
import {
  documentPointToImagePixel,
  removeColorToTransparent,
  stampRemoveBackgroundBrush,
  strokeRemoveBackgroundBrush,
} from "./imageBackground";
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

  it.each(cases.brush)("paints exact brush RGBA samples: $name", (entry) => {
    const image = {
      width: entry.width,
      height: entry.height,
      data: new Uint8ClampedArray(entry.working),
    } as ImageData;
    const original = entry.original ? {
      width: entry.width,
      height: entry.height,
      data: new Uint8ClampedArray(entry.original),
    } as ImageData : null;
    const [first, ...remaining] = entry.points;
    const mode = entry.mode as "erase" | "restore";
    let changed = stampRemoveBackgroundBrush(image, first[0], first[1], entry.radius,
      mode, original, entry.hardness);
    let previous = first;
    for (const point of remaining) {
      changed += strokeRemoveBackgroundBrush(image, previous[0], previous[1], point[0],
        point[1], entry.radius, mode, original, entry.hardness);
      previous = point;
    }
    expect(changed).toBe(entry.changed);
    expect([...image.data]).toEqual(entry.expected);
  });
});
