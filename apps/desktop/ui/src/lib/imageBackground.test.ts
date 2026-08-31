import { describe, expect, it } from "vitest";

import {
  applyImageBackgroundEdit,
  brushRadiusInNaturalPixels,
  brushStrokeDirtyRect,
  colorDistanceRgb,
  documentPointToImagePixel,
  hitTestImageElement,
  imageDataToCanvas,
  removeBgBrushScreenDiameter,
  removeColorToTransparent,
  rgbaToCss,
  rgbaToHex,
  samplePixel,
  stampRemoveBackgroundBrush,
  strokeRemoveBackgroundBrush,
  wandLoupeScreenPosition,
  WAND_LOUPE_OFFSET_PX,
  WAND_LOUPE_SIZE_PX,
} from "./imageBackground";
import {
  createScreenshotDocument,
  transformImageElement,
  withElementRotation,
  type EditorImageElement,
  type ScreenshotElement,
} from "./screenshotEditor";

/** Minimal ImageData stand-in (happy-dom may not define ImageData). */
function solidImageData(
  width: number,
  height: number,
  rgba: [number, number, number, number],
): ImageData {
  const data = new Uint8ClampedArray(width * height * 4);
  for (let index = 0; index < data.length; index += 4) {
    data[index] = rgba[0];
    data[index + 1] = rgba[1];
    data[index + 2] = rgba[2];
    data[index + 3] = rgba[3];
  }
  return { data, width, height, colorSpace: "srgb" } as ImageData;
}

function imageElement(
  overrides: Partial<EditorImageElement> = {},
): EditorImageElement {
  return {
    id: "img-1",
    kind: "image",
    source: "background",
    src: "capture.png",
    originalSrc: null,
    name: "Shot",
    sourceArtifactId: null,
    x: 0,
    y: 0,
    width: 100,
    height: 50,
    naturalWidth: 200,
    naturalHeight: 100,
    locked: true,
    visible: true,
    opacity: 100,
    blendMode: "source-over",
    ...overrides,
  };
}

describe("hitTestImageElement", () => {
  it("hits locked images and prefers the topmost layer", () => {
    const bottom = imageElement({ id: "bottom", locked: true });
    const top = imageElement({
      id: "top",
      locked: false,
      x: 10,
      y: 10,
      width: 40,
      height: 40,
    });
    const elements: ScreenshotElement[] = [bottom, top];
    expect(hitTestImageElement(elements, { x: 20, y: 20 })?.id).toBe("top");
    expect(hitTestImageElement(elements, { x: 5, y: 5 })?.id).toBe("bottom");
    expect(hitTestImageElement(elements, { x: 200, y: 200 })).toBeNull();
  });

  it("ignores hidden images", () => {
    const hidden = imageElement({ visible: false });
    expect(hitTestImageElement([hidden], { x: 10, y: 10 })).toBeNull();
  });
});

describe("documentPointToImagePixel", () => {
  it("maps document coordinates into natural pixel space", () => {
    const element = imageElement();
    expect(documentPointToImagePixel(element, { x: 0, y: 0 })).toEqual({
      x: 0,
      y: 0,
    });
    expect(documentPointToImagePixel(element, { x: 50, y: 25 })).toEqual({
      x: 100,
      y: 50,
    });
    expect(documentPointToImagePixel(element, { x: 99.9, y: 49.9 })).toEqual({
      x: 199,
      y: 99,
    });
    expect(documentPointToImagePixel(element, { x: -1, y: 0 })).toBeNull();
  });

  it("maps rotated and mirrored display coordinates back to source pixels", () => {
    const rotated = transformImageElement(imageElement(), "rotate-clockwise");
    expect(documentPointToImagePixel(rotated, { x: 25, y: -25 })).toEqual({
      x: 0,
      y: 99,
    });
    expect(documentPointToImagePixel(rotated, { x: 50, y: 25 })).toEqual({
      x: 100,
      y: 50,
    });
    expect(documentPointToImagePixel(rotated, { x: 74.9, y: 74.9 })).toEqual({
      x: 199,
      y: 0,
    });

    const mirrored = transformImageElement(imageElement(), "flip-horizontal");
    expect(documentPointToImagePixel(mirrored, { x: 0, y: 0 })).toEqual({
      x: 199,
      y: 0,
    });
    expect(documentPointToImagePixel(mirrored, { x: 99.9, y: 49.9 })).toEqual({
      x: 0,
      y: 99,
    });
  });

  it("maps freely rotated image coordinates back to source pixels", () => {
    const rotated = withElementRotation(imageElement(), Math.PI / 2);
    expect(documentPointToImagePixel(rotated, { x: 74.9, y: -24.9 })).toEqual({
      x: 0,
      y: 0,
    });
    expect(documentPointToImagePixel(rotated, { x: 50, y: 25 })).toEqual({
      x: 100,
      y: 50,
    });
    expect(hitTestImageElement([rotated], { x: 50, y: -20 })?.id).toBe("img-1");
    expect(hitTestImageElement([rotated], { x: 5, y: 25 })).toBeNull();
  });
});

describe("brushRadiusInNaturalPixels", () => {
  it("scales document brush size by the layer's display scale", () => {
    const element = imageElement({ width: 100, naturalWidth: 200 });
    // documentBrushSize is diameter-ish; radius is half after scale.
    expect(brushRadiusInNaturalPixels(element, 20)).toBeCloseTo(20, 5);
  });
});

describe("removeBgBrushScreenDiameter", () => {
  it("maps document brush diameter through zoom", () => {
    expect(removeBgBrushScreenDiameter(28, 1)).toBe(28);
    expect(removeBgBrushScreenDiameter(28, 2)).toBe(56);
    expect(removeBgBrushScreenDiameter(40, 0.5)).toBe(20);
  });
});

describe("brushStrokeDirtyRect", () => {
  it("bounds a segment and clamps it to the natural image", () => {
    expect(brushStrokeDirtyRect(100, 80, 10, 12, 30, 40, 5)).toEqual({
      x: 5,
      y: 7,
      width: 31,
      height: 39,
    });
    expect(brushStrokeDirtyRect(100, 80, 1, 1, 2, 2, 5)).toEqual({
      x: 0,
      y: 0,
      width: 8,
      height: 8,
    });
    expect(brushStrokeDirtyRect(100, 80, 10, 10, 20, 20, 0)).toBeNull();
  });
});

describe("colorDistanceRgb", () => {
  it("uses max channel delta", () => {
    expect(colorDistanceRgb(
      { r: 10, g: 20, b: 30, a: 255 },
      { r: 12, g: 0, b: 30, a: 255 },
    )).toBe(20);
  });
});

describe("removeColorToTransparent", () => {
  it("flood-fills a contiguous region", () => {
    // 3x3: center white blob on red background.
    const image = solidImageData(3, 3, [255, 0, 0, 255]);
    // Make a white pixel at (1,1) and (1,0) connected; leave corner red.
    const set = (x: number, y: number, rgba: [number, number, number, number]) => {
      const index = (y * 3 + x) * 4;
      image.data[index] = rgba[0];
      image.data[index + 1] = rgba[1];
      image.data[index + 2] = rgba[2];
      image.data[index + 3] = rgba[3];
    };
    set(1, 0, [255, 255, 255, 255]);
    set(1, 1, [255, 255, 255, 255]);

    const changed = removeColorToTransparent(image, 1, 1, 10, true);
    expect(changed).toBe(2);
    expect(samplePixel(image, 1, 1)?.a).toBe(0);
    expect(samplePixel(image, 1, 0)?.a).toBe(0);
    expect(samplePixel(image, 0, 0)?.a).toBe(255);
    expect(samplePixel(image, 0, 0)?.r).toBe(255);
  });

  it("clears all similar colors when non-contiguous", () => {
    // Seed (0,0) and a disconnected similar pixel (1,1); other cells differ.
    const image = solidImageData(2, 2, [200, 0, 0, 255]);
    image.data[0] = 10;
    image.data[1] = 10;
    image.data[2] = 10;
    image.data[3] = 255;
    image.data[12] = 12;
    image.data[13] = 8;
    image.data[14] = 10;
    image.data[15] = 255;

    const changed = removeColorToTransparent(image, 0, 0, 5, false);
    expect(changed).toBe(2);
    expect(samplePixel(image, 0, 0)?.a).toBe(0);
    expect(samplePixel(image, 1, 1)?.a).toBe(0);
    expect(samplePixel(image, 1, 0)?.a).toBe(255);
  });

  it("does nothing on already-transparent seed pixels", () => {
    const image = solidImageData(2, 2, [0, 0, 0, 0]);
    expect(removeColorToTransparent(image, 0, 0, 32, true)).toBe(0);
  });
});

describe("stampRemoveBackgroundBrush", () => {
  it("erases alpha under a hard brush", () => {
    const image = solidImageData(5, 5, [20, 40, 60, 255]);
    const changed = stampRemoveBackgroundBrush(
      image,
      2,
      2,
      1.2,
      "erase",
      null,
      1,
    );
    expect(changed).toBeGreaterThan(0);
    expect(samplePixel(image, 2, 2)?.a).toBe(0);
    // Far corner stays opaque.
    expect(samplePixel(image, 0, 0)?.a).toBe(255);
  });

  it("restores pixels from the original bitmap", () => {
    const original = solidImageData(3, 3, [10, 20, 30, 255]);
    const working = solidImageData(3, 3, [0, 0, 0, 0]);
    const changed = stampRemoveBackgroundBrush(
      working,
      1,
      1,
      2,
      "restore",
      original,
      1,
    );
    expect(changed).toBeGreaterThan(0);
    expect(samplePixel(working, 1, 1)).toEqual({
      r: 10,
      g: 20,
      b: 30,
      a: 255,
    });
  });
});

describe("strokeRemoveBackgroundBrush", () => {
  it("covers a line of pixels between two points", () => {
    const image = solidImageData(20, 5, [0, 0, 0, 255]);
    strokeRemoveBackgroundBrush(image, 1, 2, 18, 2, 1.5, "erase", null, 1);
    expect(samplePixel(image, 1, 2)?.a).toBe(0);
    expect(samplePixel(image, 10, 2)?.a).toBe(0);
    expect(samplePixel(image, 18, 2)?.a).toBe(0);
    expect(samplePixel(image, 10, 0)?.a).toBe(255);
  });
});

describe("wand loupe helpers", () => {
  it("formats sampled colors for the loupe chrome", () => {
    expect(rgbaToHex({ r: 15, g: 160, b: 255, a: 255 })).toBe("#0fa0ff");
    expect(rgbaToCss({ r: 15, g: 160, b: 255, a: 128 })).toBe("rgba(15, 160, 255, 0.502)");
    expect(rgbaToCss({ r: 0, g: 0, b: 0, a: 0 })).toBe("rgba(0, 0, 0, 0)");
  });

  it("places the loupe beside the cursor and flips near viewport edges", () => {
    const open = wandLoupeScreenPosition(40, 50, {
      viewportWidth: 800,
      viewportHeight: 600,
    });
    expect(open.left).toBe(40 + WAND_LOUPE_OFFSET_PX);
    expect(open.top).toBe(50 + WAND_LOUPE_OFFSET_PX);

    const nearCorner = wandLoupeScreenPosition(790, 590, {
      viewportWidth: 800,
      viewportHeight: 600,
    });
    // Flips above/left of the cursor and clamps inside the margin.
    expect(nearCorner.left).toBeLessThan(790);
    expect(nearCorner.top).toBeLessThan(590);
    expect(nearCorner.left + WAND_LOUPE_SIZE_PX).toBeLessThanOrEqual(800 - 8);
    expect(nearCorner.top + WAND_LOUPE_SIZE_PX).toBeLessThanOrEqual(600 - 8);
  });
});

describe("imageDataToCanvas", () => {
  it("uploads only the dirty brush rectangle when reusing a canvas", () => {
    const image = solidImageData(20, 10, [20, 40, 60, 255]);
    const canvas = document.createElement("canvas");
    canvas.width = image.width;
    canvas.height = image.height;
    const putImageData = vi.fn();
    const getContext = vi.spyOn(canvas, "getContext").mockReturnValue({
      putImageData,
    } as unknown as CanvasRenderingContext2D);

    imageDataToCanvas(image, canvas, { x: 2, y: 3, width: 5, height: 4 });

    expect(putImageData).toHaveBeenCalledOnce();
    expect(putImageData).toHaveBeenCalledWith(image, 0, 0, 2, 3, 5, 4);
    getContext.mockRestore();
  });

  it("uploads the full image when the target canvas must be resized", () => {
    const image = solidImageData(20, 10, [20, 40, 60, 255]);
    const canvas = document.createElement("canvas");
    const putImageData = vi.fn();
    const getContext = vi.spyOn(canvas, "getContext").mockReturnValue({
      putImageData,
    } as unknown as CanvasRenderingContext2D);

    imageDataToCanvas(image, canvas, { x: 2, y: 3, width: 5, height: 4 });

    expect(canvas.width).toBe(20);
    expect(canvas.height).toBe(10);
    expect(putImageData).toHaveBeenCalledOnce();
    expect(putImageData).toHaveBeenCalledWith(image, 0, 0);
    getContext.mockRestore();
  });
});

describe("applyImageBackgroundEdit", () => {
  it("freezes originalSrc, swaps the layer src, and clears solid canvas fill", () => {
    const document = createScreenshotDocument("capture.png", 100, 80);
    const next = applyImageBackgroundEdit(
      document,
      "capture-background",
      "data:image/png;base64,edited",
      "capture.png",
    );
    const image = next.elements[0];
    expect(image.kind).toBe("image");
    if (image.kind !== "image") return;
    expect(image.src).toBe("data:image/png;base64,edited");
    expect(image.originalSrc).toBe("capture.png");
    expect(next.background).toBeNull();

    const second = applyImageBackgroundEdit(
      next,
      "capture-background",
      "data:image/png;base64,edited-2",
      image.src,
    );
    const again = second.elements[0];
    expect(again.kind).toBe("image");
    if (again.kind !== "image") return;
    // Later edits keep the first frozen original for restore.
    expect(again.originalSrc).toBe("capture.png");
    expect(again.src).toBe("data:image/png;base64,edited-2");
  });
});
