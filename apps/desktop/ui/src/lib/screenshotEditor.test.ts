import {
  boundedCropRect,
  createScreenshotDocument,
  cropDocument,
  elementBounds,
  estimateCanvasExportBytes,
  expandDocumentForElement,
  hitTestElement,
  hitTestResizeHandle,
  imageSizeAtWidth,
  isSupportedImageFile,
  loadImageFile,
  outputDimensions,
  positionImportedImage,
  reorderScreenshotLayers,
  resizeBoundsFromHandle,
  resizeElement,
  translateElement,
  type EditorImageElement,
  type EditorShapeElement,
  type EditorTextElement,
} from "./screenshotEditor";

describe("screenshot editor geometry", () => {
  it("creates a lossless full-resolution document", () => {
    const document = createScreenshotDocument("captures-capture://full/capture-1", 2_560, 1_440);

    expect(document).toMatchObject({
      width: 2_560,
      height: 1_440,
      elements: [{
        kind: "image",
        source: "background",
        width: 2_560,
        height: 1_440,
      }],
    });
  });

  it("constrains a crop to the canvas and requested aspect ratio", () => {
    expect(boundedCropRect(
      { x: 900, y: 700 },
      { x: 1_400, y: 1_100 },
      { width: 1_000, height: 800 },
    )).toEqual({ x: 900, y: 700, width: 100, height: 100 });

    const widescreen = boundedCropRect(
      { x: 100, y: 100 },
      { x: 900, y: 700 },
      { width: 1_000, height: 800 },
      16 / 9,
    );
    expect(widescreen.width / widescreen.height).toBeCloseTo(16 / 9, 2);
    expect(widescreen.x + widescreen.width).toBeLessThanOrEqual(1_000);
    expect(widescreen.y + widescreen.height).toBeLessThanOrEqual(800);
  });

  it("crops by translating every editable layer", () => {
    const document = createScreenshotDocument("capture.png", 1_000, 800);
    const shape: EditorShapeElement = {
      id: "shape",
      kind: "shape",
      shape: "arrow",
      x: 200,
      y: 150,
      endX: 500,
      endY: 350,
      bend: 0,
      style: { color: "#f00", fill: null, strokeWidth: 8 },
    };
    const cropped = cropDocument(
      { ...document, elements: [...document.elements, shape] },
      { x: 100, y: 50, width: 600, height: 400 },
    );

    expect(cropped).toMatchObject({ width: 600, height: 400 });
    expect(cropped.elements[0]).toMatchObject({ x: -100, y: -50 });
    expect(cropped.elements[1]).toMatchObject({
      x: 100,
      y: 100,
      endX: 400,
      endY: 300,
    });
  });

  it("places dropped screenshots as movable layers and expands the canvas", () => {
    const document = createScreenshotDocument("capture.png", 1_000, 800);
    const position = positionImportedImage(
      2_000,
      1_000,
      document,
      { x: 900, y: 700 },
    );
    const imported: EditorImageElement = {
      id: "imported",
      kind: "image",
      source: "imported",
      src: "blob:second",
      name: "second.png",
      naturalWidth: 2_000,
      naturalHeight: 1_000,
      ...position,
    };
    const combined = expandDocumentForElement(document, imported);

    expect(imported.width).toBeLessThanOrEqual(650);
    expect(combined.elements).toHaveLength(2);
    expect(combined.width).toBeGreaterThanOrEqual(imported.x + imported.width);
    expect(hitTestElement(combined.elements, {
      x: imported.x + 10,
      y: imported.y + 10,
    })?.id).toBe("imported");
  });

  it("accepts image drops when WebKit omits the MIME type", () => {
    expect(isSupportedImageFile({ name: "reference.PNG", type: "" })).toBe(true);
    expect(isSupportedImageFile({ name: "reference", type: "image/webp" })).toBe(true);
    expect(isSupportedImageFile({ name: "notes.txt", type: "text/plain" })).toBe(false);
  });

  it("reorders editable layers front-to-back without moving the background", () => {
    const document = createScreenshotDocument("capture.png", 1_000, 800);
    const first: EditorImageElement = {
      id: "first",
      kind: "image",
      source: "imported",
      src: "blob:first",
      name: "first.png",
      x: 0,
      y: 0,
      width: 100,
      height: 100,
      naturalWidth: 100,
      naturalHeight: 100,
    };
    const second = { ...first, id: "second", src: "blob:second", name: "second.png" };
    const elements = [...document.elements, first, second];

    expect(reorderScreenshotLayers(elements, "first", "second", "before").map(({ id }) => id))
      .toEqual(["capture-background", "second", "first"]);
    expect(
      reorderScreenshotLayers(elements, "second", "capture-background", "after")
        .map(({ id }) => id),
    ).toEqual(["capture-background", "second", "first"]);
    expect(
      reorderScreenshotLayers(elements, "capture-background", "second", "before"),
    ).toBe(elements);
  });

  it("preserves imported image aspect ratio when resizing", () => {
    const image: EditorImageElement = {
      id: "image",
      kind: "image",
      source: "imported",
      src: "blob:image",
      name: "image.png",
      x: 0,
      y: 0,
      width: 400,
      height: 200,
      naturalWidth: 1_600,
      naturalHeight: 800,
    };
    expect(imageSizeAtWidth(image, 800)).toEqual({ width: 800, height: 400 });
  });

  it("moves paths and shapes without changing their geometry", () => {
    const shape: EditorShapeElement = {
      id: "shape",
      kind: "shape",
      shape: "curved_arrow",
      x: 10,
      y: 20,
      endX: 110,
      endY: 120,
      bend: 0.25,
      style: { color: "#fff", fill: null, strokeWidth: 6 },
    };
    expect(translateElement(shape, 30, -5)).toMatchObject({
      x: 40,
      y: 15,
      endX: 140,
      endY: 115,
      bend: 0.25,
    });
    expect(elementBounds(shape).width).toBeGreaterThan(100);
  });

  it("hit-tests corner resize handles and resizes bounds from the opposite corner", () => {
    const bounds = { x: 100, y: 50, width: 200, height: 100 };
    expect(hitTestResizeHandle(bounds, { x: 100, y: 50 }, 8)).toBe("nw");
    expect(hitTestResizeHandle(bounds, { x: 300, y: 150 }, 8)).toBe("se");
    expect(hitTestResizeHandle(bounds, { x: 200, y: 100 }, 8)).toBeNull();

    expect(resizeBoundsFromHandle(bounds, "se", { x: 400, y: 250 }, 8)).toEqual({
      x: 100,
      y: 50,
      width: 300,
      height: 200,
    });
    expect(resizeBoundsFromHandle(bounds, "nw", { x: 50, y: 20 }, 8)).toEqual({
      x: 50,
      y: 20,
      width: 250,
      height: 130,
    });
  });

  it("scales annotations when their selection box is resized", () => {
    const shape: EditorShapeElement = {
      id: "shape",
      kind: "shape",
      shape: "rectangle",
      x: 100,
      y: 100,
      endX: 200,
      endY: 180,
      bend: 0,
      style: { color: "#f00", fill: null, strokeWidth: 4 },
    };
    const initial = elementBounds(shape);
    const next = {
      x: initial.x,
      y: initial.y,
      width: initial.width * 2,
      height: initial.height * 2,
    };
    const resized = resizeElement(shape, initial, next);
    expect(resized).toMatchObject({
      kind: "shape",
      x: expect.any(Number),
      y: expect.any(Number),
      endX: expect.any(Number),
      endY: expect.any(Number),
    });
    if (resized.kind !== "shape") throw new Error("expected shape");
    expect(resized.endX - resized.x).toBeCloseTo((shape.endX - shape.x) * 2, 5);
    expect(resized.endY - resized.y).toBeCloseTo((shape.endY - shape.y) * 2, 5);

    const text: EditorTextElement = {
      id: "text",
      kind: "text",
      x: 40,
      y: 60,
      text: "Hi",
      fontSize: 40,
      fontFamily: "sans",
      bold: false,
      italic: false,
      align: "left",
      color: "#f00",
      background: null,
    };
    const textBounds = elementBounds(text);
    const taller = {
      ...textBounds,
      height: textBounds.height * 2,
    };
    const resizedText = resizeElement(text, textBounds, taller);
    expect(resizedText).toMatchObject({ kind: "text", fontSize: 80 });
  });

  it("calculates proportional output sizes for export", () => {
    expect(outputDimensions(2_560, 1_440, 1_280)).toEqual({
      width: 1_280,
      height: 720,
    });
  });

  it("estimates export bytes from the browser encoder", async () => {
    const canvas = document.createElement("canvas");
    canvas.width = 8;
    canvas.height = 8;
    const toBlob = vi.fn((
      callback: BlobCallback,
      type?: string,
      quality?: number,
    ) => {
      const size = type === "image/jpeg"
        ? Math.round(1_000 * (quality ?? 1))
        : 2_500;
      callback(new Blob([new Uint8Array(size)], { type: type ?? "image/png" }));
    });
    Object.defineProperty(canvas, "toBlob", { value: toBlob });

    await expect(estimateCanvasExportBytes(canvas, "png", 100)).resolves.toBe(2_500);
    await expect(estimateCanvasExportBytes(canvas, "jpeg", 70)).resolves.toBe(700);
    expect(toBlob).toHaveBeenCalledWith(expect.any(Function), "image/jpeg", 0.7);
  });

  it("loads dropped image files and falls back to a data URL when blob decode fails", async () => {
    const file = new File([new Uint8Array([1, 2, 3])], "overlay.png", {
      type: "image/png",
    });
    const createObjectURL = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:test-image");
    const revokeObjectURL = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    const originalImage = window.Image;
    let loadCount = 0;
    // @ts-expect-error test stub for Image load/decode behavior
    window.Image = class {
      onload: ((event: Event) => void) | null = null;
      onerror: ((event: Event) => void) | null = null;
      naturalWidth = 64;
      naturalHeight = 48;
      #src = "";
      get src() {
        return this.#src;
      }
      set src(value: string) {
        this.#src = value;
        loadCount += 1;
        queueMicrotask(() => {
          if (value.startsWith("blob:")) {
            this.onerror?.(new Event("error"));
            return;
          }
          this.onload?.(new Event("load"));
        });
      }
    };

    const originalFileReader = window.FileReader;
    window.FileReader = class {
      result: string | null = null;
      onload: ((event: ProgressEvent<FileReader>) => void) | null = null;
      onerror: ((event: ProgressEvent<FileReader>) => void) | null = null;
      readAsDataURL() {
        queueMicrotask(() => {
          this.result = "data:image/png;base64,aaa";
          this.onload?.(new ProgressEvent("load") as ProgressEvent<FileReader>);
        });
      }
    } as unknown as typeof FileReader;

    try {
      const image = await loadImageFile(file);
      expect(image.src).toBe("data:image/png;base64,aaa");
      expect(createObjectURL).toHaveBeenCalledWith(file);
      expect(revokeObjectURL).toHaveBeenCalledWith("blob:test-image");
      expect(loadCount).toBe(2);
    } finally {
      window.Image = originalImage;
      window.FileReader = originalFileReader;
      createObjectURL.mockRestore();
      revokeObjectURL.mockRestore();
    }
  });

  it("loads prepared drag bytes when the browser File is empty or unreadable", async () => {
    const emptyFile = new File([], "Captures_2026-07-31_15-47-00_445.png", {
      type: "image/png",
    });
    const createObjectURL = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:prepared");
    const originalImage = window.Image;
    // @ts-expect-error test stub for Image load/decode behavior
    window.Image = class {
      onload: ((event: Event) => void) | null = null;
      onerror: ((event: Event) => void) | null = null;
      naturalWidth = 32;
      naturalHeight = 24;
      #src = "";
      get src() {
        return this.#src;
      }
      set src(value: string) {
        this.#src = value;
        queueMicrotask(() => this.onload?.(new Event("load")));
      }
    };

    try {
      const image = await loadImageFile(emptyFile, {
        preparedBytes: async () => new Uint8Array([137, 80, 78, 71]),
      });
      expect(image.src).toBe("blob:prepared");
      expect(createObjectURL).toHaveBeenCalled();
      const restored = createObjectURL.mock.calls[0]?.[0] as File;
      expect(restored).toBeInstanceOf(File);
      expect(restored.name).toBe("Captures_2026-07-31_15-47-00_445.png");
      expect(restored.size).toBe(4);
    } finally {
      window.Image = originalImage;
      createObjectURL.mockRestore();
    }
  });
});
