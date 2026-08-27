import {
  applyTextStylePreset,
  arrowBendAmount,
  arrowBendFromControlPoint,
  arrowControlPoint,
  arrowDefaultMidHandle,
  arrowStarterControls,
  arrowVertices,
  arrowWithBend,
  boundedCropRect,
  cropDragAspectRatio,
  canvasOverflowEdges,
  closestImageSnapEdge,
  closestPointOnArrow,
  collectAlignmentSnapLines,
  collectEditorSourceArtifactIds,
  applyFlattenLayers,
  applyMergeLayerDown,
  applyMergeVisibleLayers,
  canFlattenLayers,
  canMergeLayerDown,
  canMergeVisibleLayers,
  createScreenshotDocument,
  cropDocument,
  curveStrokeHoverHint,
  duplicateScreenshotElement,
  elementBounds,
  LAYER_PREVIEW_SIZE,
  mergedLayerName,
  previewTransformForBounds,
  estimateCanvasExportBytes,
  expandDocumentForElement,
  imageDropExpandPadding,
  expandDocumentToFitBounds,
  previewExpandedCanvasRect,
  hitTestElement,
  hitTestArrowHandle,
  hitTestResizeHandle,
  imageDropGuideAtPoint,
  imageDropPlacementAtPoint,
  imageOrientationMatrix,
  imageSizeAtWidth,
  insertArrowControl,
  isCurveableStrokeShape,
  isSupportedImageFile,
  loadImageFile,
  outputDimensions,
  positionImportedImage,
  positionImportedImageAtEdge,
  removeArrowControl,
  reorderScreenshotLayers,
  resolveImageDropTarget,
  stackDropLightFocusAtPoint,
  resizeBoundsFromHandle,
  resizeElement,
  shiftLockedCropAspect,
  snapResizedBounds,
  snapTranslatedBounds,
  translateElement,
  canvasTrimMarginPreview,
  trimDocumentToContent,
  transformImageElement,
  TEXT_LINE_HEIGHT_RATIO,
  textStylePreset,
  visibleContentBounds,
  wrapTextLines,
  fitAutoWidthTextElement,
  fittedAutoWidthTextBox,
  isAutoWidthText,
  arrowChordLength,
  arrowHeadLength,
  scaleArrowStrokeForLength,
  type EditorImageElement,
  type EditorShapeElement,
  type EditorTextElement,
} from "./screenshotEditor";

const editableLayer = {
  locked: false,
  visible: true,
  opacity: 100,
  blendMode: "source-over" as const,
  sourceArtifactId: null as string | null,
};

function estimateLineFits(line: string, maxWidth: number, fontSize: number): boolean {
  if (!line) return true;
  return line.length * fontSize * 0.56 <= maxWidth + fontSize * 0.56;
}

describe("screenshot editor geometry", () => {
  it("creates a lossless full-resolution document", () => {
    const document = createScreenshotDocument(
      "captures-capture://full/capture-1",
      2_560,
      1_440,
      "capture-1",
    );

    expect(document).toMatchObject({
      width: 2_560,
      height: 1_440,
      background: "#f7f7f5",
      elements: [{
        kind: "image",
        source: "background",
        sourceArtifactId: "capture-1",
        width: 2_560,
        height: 1_440,
      }],
    });
    expect(collectEditorSourceArtifactIds(document.elements)).toEqual(["capture-1"]);
  });

  it("collects unique source artifact ids from image layers", () => {
    const document = createScreenshotDocument("capture.png", 100, 100, "base");
    const imported: EditorImageElement = {
      ...editableLayer,
      id: "imported",
      kind: "image",
      source: "imported",
      src: "blob:other",
      name: "other.png",
      sourceArtifactId: "other",
      x: 0,
      y: 0,
      width: 50,
      height: 50,
      naturalWidth: 50,
      naturalHeight: 50,
    };
    const orphan: EditorImageElement = {
      ...imported,
      id: "disk",
      sourceArtifactId: null,
      name: "from-disk.png",
    };
    const combined = {
      ...document,
      elements: [...document.elements, imported, orphan, { ...imported, id: "dup" }],
    };
    expect(collectEditorSourceArtifactIds(combined.elements)).toEqual(["base", "other"]);
    expect(collectEditorSourceArtifactIds(
      combined.elements.filter((element) => element.id !== "imported" && element.id !== "dup"),
    )).toEqual(["base"]);
  });

  it("allows a transparent document canvas background", () => {
    const document = {
      ...createScreenshotDocument("capture.png", 800, 600),
      background: null,
    };
    expect(document.background).toBeNull();
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

  it("snaps an off-canvas crop drag to the canvas edges", () => {
    expect(boundedCropRect(
      { x: -40, y: -20 },
      { x: 1_200, y: 900 },
      { width: 1_000, height: 800 },
    )).toEqual({ x: 0, y: 0, width: 1_000, height: 800 });

    expect(boundedCropRect(
      { x: -30, y: 120 },
      { x: 400, y: 500 },
      { width: 1_000, height: 800 },
    )).toEqual({ x: 0, y: 120, width: 400, height: 380 });
  });

  it("locks Shift-crop to the live box, then scales that ratio", () => {
    const bounds = { width: 1_000, height: 800 };
    const origin = { x: 100, y: 100 };
    const live = { x: 300, y: 200 };
    const ratio = shiftLockedCropAspect(origin, live, bounds);
    expect(ratio).toBeCloseTo(2, 5);

    const scaled = boundedCropRect(origin, { x: 500, y: 700 }, bounds, ratio);
    expect(scaled.width / scaled.height).toBeCloseTo(2, 2);
    expect(scaled.x).toBe(100);
    expect(scaled.y).toBe(100);

    expect(shiftLockedCropAspect(origin, { x: 104, y: 103 }, bounds)).toBe(1);
  });

  it("lets Shift lock the current crop ratio while a preset still wins", () => {
    const bounds = { width: 1_000, height: 800 };
    const origin = { x: 50, y: 50 };
    const current = { x: 250, y: 150 };

    const liveRect = { x: 50, y: 50, width: 200, height: 100 };
    const free = cropDragAspectRatio({
      preset: "free",
      shiftKey: true,
      origin,
      current: { x: 400, y: 600 },
      bounds,
      shiftAspect: null,
      liveRect,
    });
    expect(free.aspectRatio).toBeCloseTo(2, 5);
    expect(free.shiftAspect).toBeCloseTo(2, 5);

    const held = cropDragAspectRatio({
      preset: "free",
      shiftKey: true,
      origin,
      current: { x: 400, y: 600 },
      bounds,
      shiftAspect: free.shiftAspect,
      liveRect,
    });
    expect(held.aspectRatio).toBe(free.shiftAspect);
    expect(held.shiftAspect).toBe(free.shiftAspect);

    const released = cropDragAspectRatio({
      preset: "free",
      shiftKey: false,
      origin,
      current,
      bounds,
      shiftAspect: free.shiftAspect,
    });
    expect(released).toEqual({ aspectRatio: null, shiftAspect: null });

    const preset = cropDragAspectRatio({
      preset: "16:9",
      shiftKey: true,
      origin,
      current,
      bounds,
      shiftAspect: null,
    });
    expect(preset.aspectRatio).toBeCloseTo(16 / 9, 5);
    expect(preset.shiftAspect).toBeNull();
  });

  it("crops by translating every editable layer", () => {
    const document = createScreenshotDocument("capture.png", 1_000, 800);
    const shape: EditorShapeElement = {
      ...editableLayer,
      id: "shape",
      kind: "shape",
      shape: "arrow",
      x: 200,
      y: 150,
      endX: 500,
      endY: 350,
      controls: [],
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

  it("trims the canvas to the union of visible layer bounds", () => {
    const base = createScreenshotDocument("capture.png", 1_000, 800);
    // Shrink the original capture so empty canvas margin remains.
    const background = {
      ...base.elements[0],
      x: 80,
      y: 40,
      width: 400,
      height: 300,
    } as EditorImageElement;
    const imported: EditorImageElement = {
      ...editableLayer,
      id: "imported",
      kind: "image",
      source: "imported",
      src: "blob:import",
      name: "import.png",
      naturalWidth: 200,
      naturalHeight: 150,
      x: 420,
      y: 280,
      width: 200,
      height: 150,
    };
    const document = {
      ...base,
      elements: [background, imported],
    };

    expect(visibleContentBounds(document)).toEqual({
      x: 80,
      y: 40,
      width: 540,
      height: 390,
    });

    const trimmed = trimDocumentToContent(document);
    expect(trimmed).toMatchObject({ width: 540, height: 390 });
    expect(trimmed.elements[0]).toMatchObject({ x: 0, y: 0, width: 400, height: 300 });
    expect(trimmed.elements[1]).toMatchObject({ x: 340, y: 240, width: 200, height: 150 });
    // Already tight — identity.
    expect(trimDocumentToContent(trimmed)).toBe(trimmed);
  });

  it("ignores hidden layers when trimming and supports padding", () => {
    const base = createScreenshotDocument("capture.png", 500, 500);
    const visible: EditorImageElement = {
      ...editableLayer,
      id: "visible",
      kind: "image",
      source: "imported",
      src: "blob:a",
      name: "a.png",
      naturalWidth: 100,
      naturalHeight: 100,
      x: 50,
      y: 50,
      width: 100,
      height: 100,
    };
    const hidden: EditorImageElement = {
      ...visible,
      id: "hidden",
      name: "b.png",
      src: "blob:b",
      visible: false,
      x: 0,
      y: 0,
      width: 500,
      height: 500,
    };
    const document = {
      ...base,
      // Drop the full-bleed original so the canvas is mostly empty.
      elements: [visible, hidden],
    };

    expect(visibleContentBounds(document)).toEqual({
      x: 50,
      y: 50,
      width: 100,
      height: 100,
    });

    const padded = trimDocumentToContent(document, 10);
    expect(padded).toMatchObject({ width: 120, height: 120 });
    expect(padded.elements[0]).toMatchObject({ x: 10, y: 10 });
  });

  it("trims to the painted text background bubble, not just the glyph box", () => {
    const base = createScreenshotDocument("capture.png", 800, 600);
    const bubble: EditorTextElement = {
      ...editableLayer,
      id: "bubble",
      kind: "text",
      x: 200,
      y: 150,
      text: "Text",
      fontSize: 48,
      width: 140,
      fontFamily: "rounded",
      bold: false,
      italic: false,
      align: "center",
      color: "#ff3b5c",
      background: "#111318",
      outlined: false,
      roundedBackground: true,
    };
    const document = {
      ...base,
      elements: [{ ...base.elements[0], visible: false }, bubble],
    };

    const bounds = elementBounds(bubble);
    // Bubble pad sits outside the layout box used for wrapping.
    expect(bounds.x).toBeLessThan(bubble.x);
    expect(bounds.width).toBeGreaterThan(bubble.width);

    const trimmed = trimDocumentToContent(document);
    expect(trimmed.width).toBe(
      Math.ceil(bounds.x + bounds.width) - Math.floor(bounds.x),
    );
    expect(trimmed.height).toBe(
      Math.ceil(bounds.y + bounds.height) - Math.floor(bounds.y),
    );
    // Without background pad, trim would be too tight around the glyphs.
    const glyphOnlyWidth = bubble.width;
    expect(trimmed.width).toBeGreaterThan(glyphOnlyWidth);

    const shifted = trimmed.elements.find((element) => element.id === "bubble");
    if (shifted?.kind !== "text") throw new Error("expected text");
    // Glyph origin stays inset so the plate still has room on every side.
    expect(shifted.x).toBeGreaterThan(0);
    expect(shifted.y).toBeGreaterThan(0);
    const shiftedBounds = elementBounds(shifted);
    expect(shiftedBounds.x).toBeGreaterThanOrEqual(0);
    expect(shiftedBounds.y).toBeGreaterThanOrEqual(0);
    expect(shiftedBounds.x + shiftedBounds.width).toBeLessThanOrEqual(trimmed.width + 0.5);
    expect(shiftedBounds.y + shiftedBounds.height).toBeLessThanOrEqual(trimmed.height + 0.5);
  });

  it("keeps overhanging content when trimming", () => {
    const base = createScreenshotDocument("capture.png", 200, 200);
    const overhang: EditorImageElement = {
      ...editableLayer,
      id: "overhang",
      kind: "image",
      source: "imported",
      src: "blob:over",
      name: "over.png",
      naturalWidth: 100,
      naturalHeight: 100,
      x: -40,
      y: 20,
      width: 100,
      height: 100,
    };
    const document = {
      ...base,
      elements: [
        { ...base.elements[0], visible: false },
        overhang,
      ],
    };

    const trimmed = trimDocumentToContent(document);
    expect(trimmed).toMatchObject({ width: 100, height: 100 });
    expect(trimmed.elements[1]).toMatchObject({ x: 0, y: 0 });
  });

  it("describes on-canvas margins for the trim-edges hover preview", () => {
    const base = createScreenshotDocument("capture.png", 500, 400);
    const inset: EditorImageElement = {
      ...editableLayer,
      id: "inset",
      kind: "image",
      source: "imported",
      src: "blob:inset",
      name: "inset.png",
      naturalWidth: 200,
      naturalHeight: 150,
      x: 40,
      y: 30,
      width: 200,
      height: 150,
    };
    const document = {
      ...base,
      elements: [{ ...base.elements[0], visible: false }, inset],
    };

    const preview = canvasTrimMarginPreview(document);
    expect(preview).toEqual({
      keepRect: { x: 40, y: 30, width: 200, height: 150 },
      margins: { left: 40, top: 30, right: 260, bottom: 220 },
      edges: ["top", "right", "bottom", "left"],
    });
    expect(canvasTrimMarginPreview(trimDocumentToContent(document))).toBeNull();
  });

  it("omits overhang-only edges from the trim-edges hover preview", () => {
    const base = createScreenshotDocument("capture.png", 200, 200);
    // Content fills the canvas on three sides and overhangs left — nothing is
    // discarded from the current frame (trim expands), so no removal preview.
    const overhang: EditorImageElement = {
      ...editableLayer,
      id: "overhang",
      kind: "image",
      source: "imported",
      src: "blob:over",
      name: "over.png",
      naturalWidth: 240,
      naturalHeight: 200,
      x: -40,
      y: 0,
      width: 240,
      height: 200,
    };
    const document = {
      ...base,
      elements: [{ ...base.elements[0], visible: false }, overhang],
    };

    expect(trimDocumentToContent(document)).not.toBe(document);
    expect(canvasTrimMarginPreview(document)).toBeNull();
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
      ...editableLayer,
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
      ...editableLayer,
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

    const unlocked = elements.map((element) => ({ ...element, locked: false }));
    expect(
      reorderScreenshotLayers(unlocked, "capture-background", "second", "before")
        .map(({ id }) => id),
    ).toEqual(["first", "second", "capture-background"]);
  });

  it("snaps imports flush to the chosen layer edge and expands negative bounds", () => {
    const document = createScreenshotDocument("capture.png", 1_000, 800);
    const target = { x: 200, y: 100, width: 400, height: 300 };
    expect(closestImageSnapEdge({ x: 395, y: 405 }, target)).toBe("bottom");

    const position = positionImportedImageAtEdge(300, 200, document, target, "left");
    expect(position.x + position.width).toBe(target.x);
    const imported: EditorImageElement = {
      ...editableLayer,
      id: "left-image",
      kind: "image",
      source: "imported",
      src: "blob:left",
      name: "left.png",
      naturalWidth: 300,
      naturalHeight: 200,
      ...position,
    };
    const expanded = expandDocumentForElement(document, imported, 0);
    expect(expanded.elements.at(-1)).toMatchObject({ x: 0 });
    expect(expanded.elements[0].x).toBeGreaterThan(0);
  });

  it("keeps edge-snapped screenshots at natural 1:1 size while stack still caps overlay", () => {
    // Two captures of nearly the same UI (user's reported case).
    const document = createScreenshotDocument("capture.png", 1_307, 944);
    const target = { x: 0, y: 0, width: 1_307, height: 944 };

    const below = positionImportedImageAtEdge(1_296, 920, document, target, "bottom");
    expect(below.width).toBe(1_296);
    expect(below.height).toBe(920);
    expect(below.y).toBe(944);
    expect(below.x).toBe(Math.round((1_307 - 1_296) / 2));

    // Canvas grows flush to the import — no extra collage margin.
    const imported: EditorImageElement = {
      ...editableLayer,
      id: "below",
      kind: "image",
      source: "imported",
      src: "blob:below",
      name: "below.png",
      naturalWidth: 1_296,
      naturalHeight: 920,
      ...below,
    };
    expect(imageDropExpandPadding("bottom")).toBe(0);
    expect(imageDropExpandPadding("stack")).toBe(24);
    const expanded = expandDocumentForElement(document, imported, imageDropExpandPadding("bottom"));
    expect(expanded.height).toBe(944 + 920);
    expect(expanded.width).toBe(1_307);

    // Stack-on-top still downscales so a large overlay does not fully cover the canvas.
    const stacked = positionImportedImageAtEdge(1_296, 920, document, target, "stack");
    expect(stacked.width).toBeLessThan(1_296);
    expect(stacked.width).toBeLessThanOrEqual(Math.round(1_307 * 0.65));
  });

  it("stacks imports on the pointer and tracks an invisible light focus", () => {
    const document = createScreenshotDocument("capture.png", 1_000, 800);
    const target = { x: 200, y: 100, width: 400, height: 300 };
    // Interior of the target stacks; outer band still picks an edge.
    expect(imageDropPlacementAtPoint({ x: 400, y: 250 }, target)).toBe("stack");
    expect(imageDropPlacementAtPoint({ x: 210, y: 250 }, target)).toBe("left");
    expect(imageDropPlacementAtPoint({ x: 400, y: 110 }, target)).toBe("top");
    // Outside the rect still snaps to the closest edge.
    expect(imageDropPlacementAtPoint({ x: 100, y: 250 }, target)).toBe("left");

    // Default stack (no point) still centers on the target.
    const centered = positionImportedImageAtEdge(300, 200, document, target, "stack");
    expect(centered.x).toBe(Math.round(target.x + (target.width - centered.width) / 2));
    expect(centered.y).toBe(Math.round(target.y + (target.height - centered.height) / 2));

    // With a pointer sample, the import centers on that point.
    const point = { x: 320, y: 210 };
    const atPointer = positionImportedImageAtEdge(300, 200, document, target, "stack", point);
    expect(atPointer.x).toBe(Math.round(point.x - atPointer.width / 2));
    expect(atPointer.y).toBe(Math.round(point.y - atPointer.height / 2));

    // The estimated native-preview emitter stays compact and follows the
    // pointer without drawing a fake drag-preview tile.
    const focus = stackDropLightFocusAtPoint(point, target);
    expect(focus.width).toBeLessThan(target.width * 0.5);
    expect(focus.height).toBeLessThan(target.height * 0.5);
    expect(focus.x + focus.width / 2).toBeCloseTo(point.x, 0);
    expect(focus.y + focus.height / 2).toBeCloseTo(point.y, 0);

    const moved = stackDropLightFocusAtPoint({ x: 480, y: 300 }, target);
    expect(moved.x).toBeGreaterThan(focus.x);
    expect(moved.y).toBeGreaterThan(focus.y);
  });

  it("resolves drop snap targets without requiring a selected layer", () => {
    const document = createScreenshotDocument("capture.png", 1_000, 800);
    // No selection still snaps against the original screenshot, not a hardcoded bottom edge.
    expect(resolveImageDropTarget(document, null)).toEqual({
      x: 0,
      y: 0,
      width: 1_000,
      height: 800,
    });
    expect(imageDropGuideAtPoint(document, null, { x: 500, y: 20 }).edge).toBe("top");
    expect(imageDropGuideAtPoint(document, null, { x: 980, y: 400 }).edge).toBe("right");
    expect(imageDropGuideAtPoint(document, null, { x: 500, y: 780 }).edge).toBe("bottom");
    expect(imageDropGuideAtPoint(document, null, { x: 20, y: 400 }).edge).toBe("left");
    // Center of the canvas stacks on top of the background layer; light focus tracks the point.
    const stackGuide = imageDropGuideAtPoint(document, null, { x: 500, y: 400 });
    expect(stackGuide.edge).toBe("stack");
    expect(stackGuide.point).toEqual({ x: 500, y: 400 });
    expect(stackGuide.focus.x + stackGuide.focus.width / 2).toBeCloseTo(500, 0);
    expect(stackGuide.focus.y + stackGuide.focus.height / 2).toBeCloseTo(400, 0);

    const imported: EditorImageElement = {
      ...editableLayer,
      id: "imported",
      kind: "image",
      source: "imported",
      src: "blob:imported",
      name: "imported.png",
      x: 100,
      y: 100,
      width: 200,
      height: 150,
      naturalWidth: 200,
      naturalHeight: 150,
    };
    const layered = { ...document, elements: [...document.elements, imported] };
    // Without a pointer sample, prefer the selected image when present.
    expect(resolveImageDropTarget(layered, "imported")).toEqual({
      x: 100,
      y: 100,
      width: 200,
      height: 150,
    });
    // Otherwise use the front-most visible image.
    expect(resolveImageDropTarget(layered, null)).toEqual({
      x: 100,
      y: 100,
      width: 200,
      height: 150,
    });
    // Live drag near the imported layer's left edge / center.
    expect(imageDropGuideAtPoint(layered, "imported", { x: 50, y: 175 }).edge).toBe("left");
    expect(imageDropGuideAtPoint(layered, "imported", { x: 200, y: 175 }).edge).toBe("stack");
  });

  it("highlights only the visible layer under the pointer during image drop", () => {
    const document = createScreenshotDocument("capture.png", 1_000, 800);
    // Background fills the canvas; a smaller overlay sits on top.
    const overlay: EditorImageElement = {
      ...editableLayer,
      id: "overlay",
      kind: "image",
      source: "imported",
      src: "blob:overlay",
      name: "overlay.png",
      x: 300,
      y: 200,
      width: 400,
      height: 300,
      naturalWidth: 400,
      naturalHeight: 300,
    };
    const layered = { ...document, elements: [...document.elements, overlay] };
    const background = { x: 0, y: 0, width: 1_000, height: 800 };
    const overlayBounds = { x: 300, y: 200, width: 400, height: 300 };

    // Over the exposed background (not covered by the overlay) → background.
    expect(resolveImageDropTarget(layered, "overlay", { x: 50, y: 50 }))
      .toEqual(background);
    expect(imageDropGuideAtPoint(layered, "overlay", { x: 50, y: 50 }).target)
      .toEqual(background);

    // Over the overlay → top-most layer only (never both).
    expect(resolveImageDropTarget(layered, null, { x: 500, y: 350 }))
      .toEqual(overlayBounds);
    expect(imageDropGuideAtPoint(layered, null, { x: 500, y: 350 }).target)
      .toEqual(overlayBounds);
    expect(imageDropGuideAtPoint(layered, null, { x: 500, y: 350 }).edge)
      .toBe("stack");

    // Selection must not force a buried/off-pointer layer while hovering another.
    expect(resolveImageDropTarget(layered, "overlay", { x: 80, y: 700 }))
      .toEqual(background);

    // Just outside the overlay but still over the background → background (visible).
    expect(resolveImageDropTarget(layered, null, { x: 500, y: 190 }))
      .toEqual(background);

    // Non-overlapping layers: outside every image uses the closest one.
    const left: EditorImageElement = {
      ...editableLayer,
      id: "left",
      kind: "image",
      source: "imported",
      src: "blob:left",
      name: "left.png",
      x: 0,
      y: 0,
      width: 200,
      height: 200,
      naturalWidth: 200,
      naturalHeight: 200,
    };
    const right: EditorImageElement = {
      ...left,
      id: "right",
      src: "blob:right",
      name: "right.png",
      x: 400,
      y: 0,
    };
    const sideBySide = {
      ...document,
      elements: [left, right],
    };
    // Midway between them → front-most of the two equidistant layers.
    expect(resolveImageDropTarget(sideBySide, null, { x: 300, y: 100 }))
      .toEqual({ x: 400, y: 0, width: 200, height: 200 });
    // Closer to the left layer's right edge.
    expect(resolveImageDropTarget(sideBySide, null, { x: 250, y: 100 }))
      .toEqual({ x: 0, y: 0, width: 200, height: 200 });
    expect(imageDropGuideAtPoint(sideBySide, null, { x: 250, y: 100 }).edge)
      .toBe("right");
  });

  it("duplicates layers as visible unlocked imports", () => {
    const background = createScreenshotDocument("capture.png", 1_000, 800).elements[0];
    const copy = duplicateScreenshotElement(background, "copy", 12);
    expect(copy).toMatchObject({
      id: "copy",
      kind: "image",
      source: "imported",
      x: 12,
      y: 12,
      locked: false,
      visible: true,
    });
  });

  it("only hit-tests visible unlocked layers for move-hover affordances", () => {
    const document = createScreenshotDocument("capture.png", 1_000, 800);
    expect(hitTestElement(document.elements, { x: 20, y: 20 })).toBeNull();
    const background = { ...document.elements[0], locked: false };
    expect(hitTestElement([background], { x: 20, y: 20 })?.id).toBe("capture-background");
    expect(hitTestElement([{ ...background, visible: false }], { x: 20, y: 20 }))
      .toBeNull();
  });

  it("preserves imported image aspect ratio when resizing", () => {
    const image: EditorImageElement = {
      ...editableLayer,
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

  it("rotates and flips image layers losslessly around their center", () => {
    const image: EditorImageElement = {
      ...editableLayer,
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

    const clockwise = transformImageElement(image, "rotate-clockwise");
    expect(clockwise).toMatchObject({
      x: 100,
      y: -100,
      width: 200,
      height: 400,
    });
    expect(imageOrientationMatrix(clockwise.orientation)).toEqual({
      a: 0,
      b: 1,
      c: -1,
      d: 0,
    });
    expect(imageSizeAtWidth(clockwise, 200)).toEqual({ width: 200, height: 400 });

    const mirrored = transformImageElement(clockwise, "flip-horizontal");
    expect(imageOrientationMatrix(mirrored.orientation)).toEqual({
      a: 0,
      b: 1,
      c: 1,
      d: 0,
    });

    const restored = [0, 1, 2, 3].reduce(
      (current) => transformImageElement(current, "rotate-clockwise"),
      image,
    );
    expect(restored).toMatchObject({
      x: 0,
      y: 0,
      width: 400,
      height: 200,
    });
    expect(restored.orientation).toBeUndefined();
  });

  it("moves paths and shapes without changing their geometry", () => {
    const shape: EditorShapeElement = {
      ...editableLayer,
      id: "shape",
      kind: "shape",
      shape: "arrow",
      x: 10,
      y: 20,
      endX: 110,
      endY: 120,
      controls: [{ x: 70, y: 90 }],
      style: { color: "#fff", fill: null, strokeWidth: 6 },
    };
    expect(translateElement(shape, 30, -5)).toMatchObject({
      x: 40,
      y: 15,
      endX: 140,
      endY: 115,
      controls: [{ x: 100, y: 85 }],
    });
    expect(elementBounds(shape).width).toBeGreaterThan(100);
  });

  it("uses one arrow model for straight, single-curve, and multi-point arrows", () => {
    const arrow: EditorShapeElement = {
      ...editableLayer,
      id: "arrow",
      kind: "shape",
      shape: "arrow",
      x: 100,
      y: 100,
      endX: 300,
      endY: 100,
      controls: [],
      style: { color: "#fff", fill: null, strokeWidth: 6 },
    };

    expect(arrowVertices(arrow)).toEqual([
      { x: 100, y: 100 },
      { x: 300, y: 100 },
    ]);
    expect(arrowDefaultMidHandle(arrow)).toEqual({ x: 200, y: 100 });
    expect(arrowStarterControls(arrow)).toEqual([
      { x: 150, y: 100 },
      { x: 200, y: 100 },
      { x: 250, y: 100 },
    ]);
    expect(arrowControlPoint(arrow)).toEqual({ x: 200, y: 100 });
    expect(hitTestArrowHandle(arrow, { x: 155, y: 104 }, 8)).toEqual({
      kind: "starter-control",
      index: 0,
    });
    expect(hitTestArrowHandle(arrow, { x: 205, y: 104 }, 8)).toEqual({
      kind: "starter-control",
      index: 1,
    });
    expect(hitTestArrowHandle(arrow, { x: 255, y: 104 }, 8)).toEqual({
      kind: "starter-control",
      index: 2,
    });
    expect(hitTestArrowHandle(arrow, { x: 100, y: 100 }, 8)).toEqual({ kind: "start" });
    expect(hitTestArrowHandle(arrow, { x: 300, y: 100 }, 8)).toEqual({ kind: "end" });
    expect(arrowBendFromControlPoint(arrow, { x: 200, y: 200 })).toBeCloseTo(0.5);
    expect(arrowBendFromControlPoint(arrow, { x: 200, y: -300 })).toBe(-1);

    const bent = arrowWithBend(arrow, 0.5);
    expect(bent.controls).toEqual([{ x: 200, y: 200 }]);
    expect(arrowControlPoint(bent)).toEqual({ x: 200, y: 200 });
    expect(arrowBendAmount(bent)).toBeCloseTo(0.5);
    expect(elementBounds(bent).height).toBeGreaterThan(elementBounds(arrow).height);
    expect(hitTestArrowHandle(bent, { x: 200, y: 200 }, 8)).toEqual({
      kind: "control",
      index: 0,
    });

    const withTwo = insertArrowControl(bent, { x: 250, y: 150 });
    expect(withTwo?.controls).toHaveLength(2);
    expect(removeArrowControl(withTwo!, 0).controls).toHaveLength(1);

    let manyControls: EditorShapeElement = arrow;
    for (let i = 0; i < 12; i += 1) {
      const next = insertArrowControl(manyControls, {
        x: 120 + i * 20,
        y: 140 + i * 5,
      });
      if (next) manyControls = next;
    }
    expect(manyControls.controls).toHaveLength(12);
    expect(insertArrowControl(manyControls, { x: 200, y: 180 })?.controls).toHaveLength(13);

    const near = closestPointOnArrow(arrow, { x: 200, y: 108 });
    expect(near.distance).toBeLessThan(10);
    expect(near.point.x).toBeCloseTo(200, 0);
  });

  it("shares multi-point curve controls with lines", () => {
    expect(isCurveableStrokeShape("line")).toBe(true);
    expect(isCurveableStrokeShape("arrow")).toBe(true);
    expect(isCurveableStrokeShape("ellipse")).toBe(false);
    expect(isCurveableStrokeShape("rectangle")).toBe(false);

    const line: EditorShapeElement = {
      ...editableLayer,
      id: "line",
      kind: "shape",
      shape: "line",
      x: 50,
      y: 80,
      endX: 250,
      endY: 80,
      controls: [],
      style: { color: "#0af", fill: null, strokeWidth: 4 },
    };

    expect(hitTestArrowHandle(line, { x: 150, y: 80 }, 8)).toEqual({
      kind: "starter-control",
      index: 1,
    });
    const bent = arrowWithBend(line, 0.25);
    expect(bent.controls).toHaveLength(1);
    expect(arrowBendAmount(bent)).toBeCloseTo(0.25);
    expect(elementBounds(bent).height).toBeGreaterThan(elementBounds(line).height);

    const withPoint = insertArrowControl(bent, { x: 200, y: 100 });
    expect(withPoint?.controls).toHaveLength(2);
    expect(removeArrowControl(withPoint!, 1).controls).toHaveLength(1);

    expect(curveStrokeHoverHint(line, { x: 150, y: 80 }, 8)).toMatch(/Drag a dot to curve/);
    expect(curveStrokeHoverHint(bent, { x: 150, y: 130 }, 8)).toMatch(/Double-click to remove/);
    expect(curveStrokeHoverHint(bent, { x: 200, y: 95 }, 8)).toMatch(/Double-click to add a curve point/);
    expect(curveStrokeHoverHint(line, { x: 10, y: 10 }, 8)).toBeNull();
  });

  it("bounds arrows by path geometry so empty curve sides do not block trim", () => {
    // Horizontal shaft, single free control only downward.
    // Quadratic apex is at t=0.5 → y = 0.25*200 + 0.5*300 + 0.25*200 = 250,
    // not the off-path control at y=300.
    const arrow: EditorShapeElement = {
      ...editableLayer,
      id: "arrow",
      kind: "shape",
      shape: "arrow",
      x: 100,
      y: 200,
      endX: 300,
      endY: 200,
      controls: [{ x: 200, y: 300 }],
      style: { color: "#f00", fill: null, strokeWidth: 4 },
    };
    expect(arrowControlPoint(arrow)).toEqual({ x: 200, y: 300 });
    const bounds = elementBounds(arrow);
    // Only stroke/head padding may sit above y=200; the old isotropic curve
    // pad would place the top near y=200 - length*|bend| ≈ 100 or lower.
    expect(bounds.y).toBeGreaterThan(170);
    // Painted apex is ~250; free control at 300 must not inflate the box.
    expect(bounds.y + bounds.height).toBeGreaterThan(250);
    expect(bounds.y + bounds.height).toBeLessThan(280);
    // Empty left/right of the shaft should not get a full curve-radius pad.
    expect(bounds.x).toBeGreaterThan(70);
    expect(bounds.x + bounds.width).toBeLessThan(330);

    // Canvas grown around a small capture + this bent arrow: empty margin
    // above the content must remain trimmable.
    const base = createScreenshotDocument("capture.png", 500, 500);
    const document = {
      ...base,
      elements: [
        {
          ...base.elements[0],
          x: 150,
          y: 180,
          width: 100,
          height: 40,
        } as EditorImageElement,
        arrow,
      ],
    };
    const content = visibleContentBounds(document);
    expect(content).not.toBeNull();
    expect(content!.y).toBeGreaterThan(0);
    const trimmed = trimDocumentToContent(document);
    expect(trimmed.height).toBeLessThan(document.height);
    expect(trimmed.width).toBeLessThan(document.width);
  });

  it("does not expand or block trim when only an off-path control sits past the edge", () => {
    // Endpoints and painted curve stay inside 400×300; the free control is
    // well below the bottom edge. Expand and trim must ignore that handle.
    const arrow: EditorShapeElement = {
      ...editableLayer,
      id: "arrow",
      kind: "shape",
      shape: "arrow",
      x: 80,
      y: 120,
      endX: 320,
      endY: 120,
      controls: [{ x: 200, y: 420 }],
      style: { color: "#f00", fill: null, strokeWidth: 4 },
    };
    const document = {
      ...createScreenshotDocument("capture.png", 400, 300),
      elements: [arrow],
    };

    const bounds = elementBounds(arrow);
    expect(bounds.y).toBeGreaterThan(0);
    // Quadratic apex y = 0.25*120 + 0.5*420 + 0.25*120 = 270 (inside 300).
    expect(bounds.y + bounds.height).toBeLessThan(300);
    expect(bounds.y + bounds.height).toBeGreaterThan(260);
    // Control sits at y=420 — must not be part of content bounds.
    expect(bounds.y + bounds.height).toBeLessThan(420);

    expect(canvasOverflowEdges(bounds, document)).toEqual([]);
    expect(previewExpandedCanvasRect(bounds, document)).toBeNull();
    expect(expandDocumentToFitBounds(document, bounds, 0)).toBe(document);

    // Extra empty margin under the curve (canvas taller than content) trims.
    const tall = { ...document, height: 500 };
    const trimmed = trimDocumentToContent(tall);
    expect(trimmed.height).toBeLessThan(tall.height);
    expect(trimmed.height).toBeLessThan(320);
  });

  it("bounds multi-point arrows from the painted path, not free vertices", () => {
    // Free vertices act as quadratic controls and sit off the stroke.
    const arrow: EditorShapeElement = {
      ...editableLayer,
      id: "arrow",
      kind: "shape",
      shape: "arrow",
      x: 50,
      y: 200,
      endX: 350,
      endY: 200,
      controls: [
        { x: 120, y: 40 },
        { x: 280, y: 360 },
      ],
      style: { color: "#f00", fill: null, strokeWidth: 4 },
    };
    const bounds = elementBounds(arrow);
    // Off-path free vertices at y=40 / y=360 must not define the box alone;
    // samples pull the hull inward toward the actual segments.
    expect(bounds.y).toBeGreaterThan(40);
    expect(bounds.y + bounds.height).toBeLessThan(360);
  });

  it("bounds lines and boxes by stroke extent without phantom padding", () => {
    const line: EditorShapeElement = {
      ...editableLayer,
      id: "line",
      kind: "shape",
      shape: "line",
      x: 50,
      y: 50,
      endX: 150,
      endY: 50,
      controls: [],
      style: { color: "#0f0", fill: null, strokeWidth: 4 },
    };
    const lineBounds = elementBounds(line);
    expect(lineBounds.y).toBeGreaterThan(40);
    expect(lineBounds.height).toBeLessThan(20);

    const rect: EditorShapeElement = {
      ...line,
      id: "rect",
      shape: "rectangle",
      endY: 120,
    };
    const rectBounds = elementBounds(rect);
    // strokeExtent(4) = ceil(2) + 1 = 3
    expect(rectBounds).toMatchObject({ x: 47, y: 47, width: 106, height: 76 });
  });

  it("pads arrow heads at the tip only so empty shaft sides stay tight for trim", () => {
    // Tall vertical arrow: isotropic head pad used to inflate the start end
    // (bottom) by ~half the wing length even though wings only sit near the tip.
    const vertical: EditorShapeElement = {
      ...editableLayer,
      id: "arrow-v",
      kind: "shape",
      shape: "arrow",
      x: 200,
      y: 300,
      endX: 200,
      endY: 80,
      controls: [],
      style: { color: "#f00", fill: null, strokeWidth: 8 },
    };
    const verticalBounds = elementBounds(vertical);
    // Bottom of the shaft stays near y=300; old isotropic pad pushed it to ~325+.
    expect(verticalBounds.y + verticalBounds.height).toBeLessThan(310);
    // Tip + wings still expand past the endpoint (and laterally near the tip).
    expect(verticalBounds.y).toBeLessThan(80);
    expect(verticalBounds.width).toBeGreaterThan(20);

    // Horizontal shaft, tip on the right: left (start) must not get head pad.
    const horizontal: EditorShapeElement = {
      ...vertical,
      id: "arrow-h",
      x: 80,
      y: 200,
      endX: 320,
      endY: 200,
    };
    const horizontalBounds = elementBounds(horizontal);
    // strokeExtent(8) = 5 → left edge ≈ 75. Old isotropic head pad ≈ 24 → ~56.
    expect(horizontalBounds.x).toBeGreaterThan(70);
    expect(horizontalBounds.x).toBeLessThan(80);
    // Right edge still includes the tip and wings.
    expect(horizontalBounds.x + horizontalBounds.width).toBeGreaterThan(320);

    const line: EditorShapeElement = { ...vertical, id: "line", shape: "line" };
    const lineBounds = elementBounds(line);
    // Lines have no head: box is shaft + stroke only.
    expect(lineBounds.y).toBeGreaterThan(70);
    expect(lineBounds.y + lineBounds.height).toBeLessThan(310);
    expect(lineBounds.width).toBeLessThan(15);
  });

  it("hit-tests corner and edge resize handles and resizes from the opposite side", () => {
    const bounds = { x: 100, y: 50, width: 200, height: 100 };
    expect(hitTestResizeHandle(bounds, { x: 100, y: 50 }, 8)).toBe("nw");
    expect(hitTestResizeHandle(bounds, { x: 300, y: 150 }, 8)).toBe("se");
    expect(hitTestResizeHandle(bounds, { x: 200, y: 50 }, 8)).toBe("n");
    expect(hitTestResizeHandle(bounds, { x: 300, y: 100 }, 8)).toBe("e");
    // Interior of the selection is not a resize grip.
    expect(hitTestResizeHandle(bounds, { x: 200, y: 100 }, 8)).toBeNull();
    // Dragging the dashed border strip (not just the mid grip) also counts.
    expect(hitTestResizeHandle(bounds, { x: 180, y: 50 }, 8)).toBe("n");

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
    expect(resizeBoundsFromHandle(bounds, "e", { x: 350, y: 90 }, 8)).toEqual({
      x: 100,
      y: 50,
      width: 250,
      height: 100,
    });
    expect(resizeBoundsFromHandle(bounds, "n", { x: 150, y: 20 }, 8)).toEqual({
      x: 100,
      y: 20,
      width: 200,
      height: 130,
    });
  });

  it("locks aspect ratio when resizing from a corner with Shift", () => {
    // 2:1 initial box. Freehand SE to (400,250) is 300×200 (3:2); lock stays 2:1.
    const bounds = { x: 100, y: 50, width: 200, height: 100 };
    // Height dominates (free 1.5 < aspect 2) → 400×200.
    const lockedSeTall = resizeBoundsFromHandle(bounds, "se", { x: 400, y: 250 }, 8, true);
    expect(lockedSeTall).toMatchObject({ x: 100, y: 50 });
    expect(lockedSeTall.width / lockedSeTall.height).toBeCloseTo(2, 5);
    expect(lockedSeTall.width).toBeCloseTo(400, 5);
    expect(lockedSeTall.height).toBeCloseTo(200, 5);

    // Width dominates (free 300×50 is 6:1 > 2) → 300×150.
    const lockedSeWide = resizeBoundsFromHandle(bounds, "se", { x: 400, y: 100 }, 8, true);
    expect(lockedSeWide).toMatchObject({ x: 100, y: 50 });
    expect(lockedSeWide.width / lockedSeWide.height).toBeCloseTo(2, 5);
    expect(lockedSeWide.width).toBeCloseTo(300, 5);
    expect(lockedSeWide.height).toBeCloseTo(150, 5);

    // NW corner: opposite SE stays fixed; aspect preserved.
    const lockedNw = resizeBoundsFromHandle(bounds, "nw", { x: 50, y: 0 }, 8, true);
    expect(lockedNw.x + lockedNw.width).toBeCloseTo(300, 5);
    expect(lockedNw.y + lockedNw.height).toBeCloseTo(150, 5);
    expect(lockedNw.width / lockedNw.height).toBeCloseTo(2, 5);

    // Mid-edge handles ignore the lock (single-axis still free).
    expect(resizeBoundsFromHandle(bounds, "e", { x: 350, y: 90 }, 8, true)).toEqual({
      x: 100,
      y: 50,
      width: 250,
      height: 100,
    });
  });

  it("snaps moved layers to other image edges and the canvas border", () => {
    const document = createScreenshotDocument("capture.png", 1_000, 800);
    const imported: EditorImageElement = {
      ...editableLayer,
      id: "imported",
      kind: "image",
      source: "imported",
      src: "blob:imported",
      name: "imported.png",
      x: 200,
      y: 100,
      width: 100,
      height: 80,
      naturalWidth: 100,
      naturalHeight: 80,
    };
    const layered = { ...document, elements: [...document.elements, imported] };
    const lines = collectAlignmentSnapLines(layered, "moving");
    expect(lines.vertical).toEqual(expect.arrayContaining([0, 1_000, 200, 300]));
    expect(lines.horizontal).toEqual(expect.arrayContaining([0, 800, 100, 180]));

    // Near the right edge of the imported layer.
    const snapped = snapTranslatedBounds(
      { x: 305, y: 40, width: 50, height: 40 },
      lines,
      10,
    );
    expect(snapped.bounds.x).toBe(300);
    expect(snapped.guides).toContainEqual({ orientation: "vertical", position: 300 });

    // Near the top canvas border.
    const toCanvas = snapTranslatedBounds(
      { x: 40, y: 6, width: 50, height: 40 },
      lines,
      10,
    );
    expect(toCanvas.bounds.y).toBe(0);
    expect(toCanvas.guides).toContainEqual({ orientation: "horizontal", position: 0 });
  });

  it("snaps resized edges against neighboring layers", () => {
    const lines = {
      vertical: [0, 400, 1_000],
      horizontal: [0, 300, 800],
    };
    const initial = { x: 100, y: 100, width: 200, height: 120 };
    const free = resizeBoundsFromHandle(initial, "se", { x: 405, y: 295 }, 8);
    const snapped = snapResizedBounds(initial, "se", free, lines, 10, 8);
    expect(snapped.bounds).toMatchObject({ x: 100, y: 100, width: 300, height: 200 });
    expect(snapped.guides).toEqual(expect.arrayContaining([
      { orientation: "vertical", position: 400 },
      { orientation: "horizontal", position: 300 },
    ]));
  });

  it("detects canvas overflow and expands the document to fit bounds", () => {
    const document = createScreenshotDocument("capture.png", 1_000, 800);
    const overflowing = { x: -40, y: 100, width: 200, height: 100 };
    expect(canvasOverflowEdges(overflowing, document)).toEqual(["left"]);
    expect(canvasOverflowEdges(
      { x: 900, y: 750, width: 200, height: 100 },
      document,
    )).toEqual(["right", "bottom"]);

    expect(previewExpandedCanvasRect(overflowing, document)).toEqual({
      x: -40,
      y: 0,
      width: 1_040,
      height: 800,
    });
    expect(previewExpandedCanvasRect(
      { x: 900, y: 750, width: 200, height: 100 },
      document,
    )).toEqual({
      x: 0,
      y: 0,
      width: 1_100,
      height: 850,
    });
    expect(previewExpandedCanvasRect({
      x: 10,
      y: 10,
      width: 100,
      height: 100,
    }, document)).toBeNull();

    const expanded = expandDocumentToFitBounds(document, overflowing, 0);
    expect(expanded.width).toBe(1_040);
    expect(expanded.height).toBe(800);
    expect(expanded.elements[0]).toMatchObject({ x: 40, y: 0 });
    expect(expandDocumentToFitBounds(document, {
      x: 10,
      y: 10,
      width: 100,
      height: 100,
    })).toBe(document);
  });

  it("scales annotations when their selection box is resized", () => {
    const shape: EditorShapeElement = {
      ...editableLayer,
      id: "shape",
      kind: "shape",
      shape: "rectangle",
      x: 100,
      y: 100,
      endX: 200,
      endY: 180,
      controls: [],
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
    expect(resized.style.strokeWidth).toBe(4);

    const arrow: EditorShapeElement = {
      ...editableLayer,
      id: "arrow",
      kind: "shape",
      shape: "arrow",
      x: 80,
      y: 80,
      endX: 280,
      endY: 180,
      controls: [],
      style: { color: "#f00", fill: null, strokeWidth: 8 },
    };
    const arrowBounds = elementBounds(arrow);
    const shrunkArrow = resizeElement(arrow, arrowBounds, {
      x: arrowBounds.x,
      y: arrowBounds.y,
      width: arrowBounds.width * 0.25,
      height: arrowBounds.height * 0.25,
    });
    if (shrunkArrow.kind !== "shape") throw new Error("expected arrow");
    expect(shrunkArrow.style.strokeWidth).toBeCloseTo(2, 5);
    expect(arrowHeadLength(shrunkArrow.style.strokeWidth, arrowChordLength(shrunkArrow)))
      .toBeLessThan(arrowHeadLength(arrow.style.strokeWidth, arrowChordLength(arrow)) * 0.5);

    const text: EditorTextElement = {
      ...editableLayer,
      id: "text",
      kind: "text",
      x: 40,
      y: 60,
      text: "Hi there friend",
      fontSize: 40,
      width: 120,
      fontFamily: "sans",
      bold: false,
      italic: false,
      align: "left",
      color: "#f00",
      background: null,
      outlined: false,
      roundedBackground: false,
    };
    const textBounds = elementBounds(text);
    // Stretching taller scales type size so the selection box drag is visible.
    const taller = {
      ...textBounds,
      height: textBounds.height * 2,
    };
    const resizedTaller = resizeElement(text, textBounds, taller);
    expect(resizedTaller).toMatchObject({
      kind: "text",
      fontSize: 80,
      width: 240,
      x: textBounds.x,
      y: textBounds.y,
    });

    const wider = {
      ...textBounds,
      width: textBounds.width * 2,
    };
    const resizedWider = resizeElement(text, textBounds, wider);
    expect(resizedWider).toMatchObject({
      kind: "text",
      fontSize: 40,
      width: 240,
    });
    // Wider box reflows to fewer lines (or equal when already single-line).
    expect(elementBounds(resizedWider as EditorTextElement).height)
      .toBeLessThanOrEqual(elementBounds(text).height);

    const autoWidth: EditorTextElement = {
      ...text,
      id: "auto",
      autoWidth: true,
      width: fittedAutoWidthTextBox("Hi", 40),
      text: "Hi",
    };
    const autoTaller = resizeElement(autoWidth, elementBounds(autoWidth), {
      ...elementBounds(autoWidth),
      height: elementBounds(autoWidth).height * 2,
    });
    expect(autoTaller).toMatchObject({ kind: "text", autoWidth: true, fontSize: 80 });
    if (autoTaller.kind !== "text") throw new Error("expected text");
    expect(autoTaller.width).toBe(fittedAutoWidthTextBox("Hi", 80));

    const autoWider = resizeElement(autoWidth, elementBounds(autoWidth), {
      ...elementBounds(autoWidth),
      width: elementBounds(autoWidth).width * 2,
    });
    expect(autoWider).toMatchObject({ kind: "text", autoWidth: true, fontSize: 80 });
    if (autoWider.kind !== "text") throw new Error("expected text");
    expect(autoWider.width).toBe(fittedAutoWidthTextBox("Hi", 80));

    // Non-uniform corner drag must not stretch the plate independently of type.
    const bubbleLabel: EditorTextElement = {
      ...autoWidth,
      id: "bubble-label",
      background: "#111318",
      roundedBackground: true,
      fontFamily: "rounded",
      align: "center",
    };
    const bubbleStart = elementBounds(bubbleLabel);
    const stretched = resizeElement(bubbleLabel, bubbleStart, {
      ...bubbleStart,
      width: bubbleStart.width * 2,
      height: bubbleStart.height * 1.1,
    });
    expect(stretched).toMatchObject({ kind: "text", autoWidth: true, fontSize: 44 });
    if (stretched.kind !== "text") throw new Error("expected text");
    const bubbleEnd = elementBounds(stretched);
    expect(bubbleEnd.width / bubbleEnd.height)
      .toBeCloseTo(bubbleStart.width / bubbleStart.height, 2);
    expect(bubbleEnd.width / bubbleEnd.height)
      .toBeLessThan((bubbleStart.width * 2) / (bubbleStart.height * 1.1) * 0.7);

    // Background plates expand paint/selection bounds beyond the layout box.
    const bubble: EditorTextElement = {
      ...text,
      id: "bubble",
      background: "#111318",
      roundedBackground: true,
      fontFamily: "rounded",
      align: "center",
    };
    const bubbleBounds = elementBounds(bubble);
    expect(bubbleBounds.x).toBeLessThan(bubble.x);
    expect(bubbleBounds.y).toBeLessThan(bubble.y);
    expect(bubbleBounds.width).toBeGreaterThan(bubble.width);
    expect(bubbleBounds.height).toBeGreaterThan(
      Math.max(1, wrapTextLines(bubble.text, bubble.width, bubble.fontSize).length)
        * bubble.fontSize
        * TEXT_LINE_HEIGHT_RATIO,
    );
  });

  it("wraps text within the box width without scaling font size", () => {
    const wrapped = wrapTextLines("one two three four", 80, 20);
    expect(wrapped.length).toBeGreaterThan(1);
    expect(wrapped.every((line) => estimateLineFits(line, 80, 20))).toBe(true);

    const withBreaks = wrapTextLines("hello\nworld", 400, 20);
    expect(withBreaks).toEqual(["hello", "world"]);

    const hard = wrapTextLines("supercalifragilistic", 40, 20);
    expect(hard.length).toBeGreaterThan(1);
    expect(hard.join("")).toBe("supercalifragilistic");
  });

  it("grows auto-width text horizontally instead of wrapping into a column", () => {
    const short: EditorTextElement = {
      ...editableLayer,
      id: "label",
      kind: "text",
      x: 40,
      y: 60,
      text: "Hi",
      fontSize: 40,
      width: fittedAutoWidthTextBox("Hi", 40),
      autoWidth: true,
      fontFamily: "sans",
      bold: false,
      italic: false,
      align: "left",
      color: "#f00",
      background: null,
      outlined: false,
      roundedBackground: false,
    };
    expect(isAutoWidthText(short)).toBe(true);

    const long = fitAutoWidthTextElement({
      ...short,
      text: "Hello from the screenshot editor",
    });
    expect(long.width).toBeGreaterThan(short.width);
    expect(wrapTextLines(long.text, long.width, long.fontSize)).toEqual([
      "Hello from the screenshot editor",
    ]);
    expect(elementBounds(long).height).toBeCloseTo(elementBounds(short).height, 5);

    const centered = fitAutoWidthTextElement({
      ...short,
      align: "center",
      text: "Hello from the screenshot editor",
    });
    expect(centered.x).toBeLessThan(short.x);

    const wrapped = fitAutoWidthTextElement({
      ...short,
      autoWidth: false,
      width: 80,
      text: "Hello from the screenshot editor",
    });
    expect(wrapped.width).toBe(80);
    expect(wrapTextLines(wrapped.text, wrapped.width, wrapped.fontSize).length)
      .toBeGreaterThan(1);
  });

  it("scales arrow heads with stroke and shaft length", () => {
    expect(arrowHeadLength(8)).toBeCloseTo(33.6, 5);
    expect(arrowHeadLength(2)).toBeCloseTo(8.4, 5);
    expect(arrowHeadLength(8, 20)).toBeCloseTo(4.4, 5);
    expect(arrowHeadLength(8, 400)).toBeCloseTo(33.6, 5);

    const long: EditorShapeElement = {
      ...editableLayer,
      id: "arrow",
      kind: "shape",
      shape: "arrow",
      x: 0,
      y: 0,
      endX: 200,
      endY: 0,
      controls: [],
      style: { color: "#f00", fill: null, strokeWidth: 8 },
    };
    const short = scaleArrowStrokeForLength(long, { ...long, endX: 50 });
    expect(short.style.strokeWidth).toBeCloseTo(2, 5);
    expect(arrowHeadLength(short.style.strokeWidth, arrowChordLength(short)))
      .toBeLessThan(12);
    const longer = scaleArrowStrokeForLength(long, { ...long, endX: 400 });
    expect(longer.style.strokeWidth).toBe(8);
  });

  it("applies all named text styles without changing content or color", () => {
    const text: EditorTextElement = {
      ...editableLayer,
      id: "text-style",
      kind: "text",
      x: 20,
      y: 30,
      text: "Keep me",
      fontSize: 40,
      width: 180,
      fontFamily: "serif",
      bold: true,
      italic: false,
      align: "center",
      color: "#ff3b5c",
      background: null,
      outlined: false,
      roundedBackground: false,
    };

    const expectations = {
      standard: { fontFamily: "sans", background: null, outlined: false, roundedBackground: false },
      rounded: { fontFamily: "rounded", background: null, outlined: false, roundedBackground: false },
      outlined: { fontFamily: "sans", background: null, outlined: true, roundedBackground: false },
      mono: { fontFamily: "mono", background: null, outlined: false, roundedBackground: false },
      box: { fontFamily: "sans", background: "#111318", outlined: false, roundedBackground: false },
      "mono-box": { fontFamily: "mono", background: "#111318", outlined: false, roundedBackground: false },
      "rounded-box": { fontFamily: "rounded", background: "#111318", outlined: false, roundedBackground: true },
    } as const;

    for (const [preset, expected] of Object.entries(expectations)) {
      const styled = applyTextStylePreset(text, preset as keyof typeof expectations);
      expect(styled).toMatchObject({
        ...expected,
        text: "Keep me",
        color: "#ff3b5c",
        fontSize: 40,
        bold: true,
      });
      expect(textStylePreset(styled)).toBe(preset);
    }
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

describe("layer merge and flatten helpers", () => {
  const baseImage = (id: string, name: string, overrides: Partial<EditorImageElement> = {}): EditorImageElement => ({
    id,
    kind: "image",
    source: "imported",
    src: `data:${id}`,
    name,
    x: 0,
    y: 0,
    width: 100,
    height: 80,
    naturalWidth: 100,
    naturalHeight: 80,
    ...editableLayer,
    ...overrides,
  });

  const textLayer = (id: string, text: string, overrides: Partial<EditorTextElement> = {}): EditorTextElement => ({
    id,
    kind: "text",
    text,
    fontSize: 18,
    width: 120,
    fontFamily: "sans",
    bold: false,
    italic: false,
    align: "left",
    color: "#ffffff",
    background: null,
    outlined: false,
    roundedBackground: false,
    x: 10,
    y: 10,
    ...editableLayer,
    ...overrides,
  });

  it("gates merge down on an unlocked layer with an unlocked neighbor below", () => {
    const layers = [
      baseImage("bg", "Background", { locked: true, source: "background" }),
      textLayer("t1", "Hello"),
      textLayer("t2", "World"),
    ];
    expect(canMergeLayerDown(layers, "bg")).toBe(false);
    expect(canMergeLayerDown(layers, "t1")).toBe(false); // below is locked
    expect(canMergeLayerDown(layers, "t2")).toBe(true);
    expect(canMergeLayerDown(layers, null)).toBe(false);
  });

  it("gates merge visible and flatten on stack size", () => {
    const single = [baseImage("a", "A")];
    const two = [baseImage("a", "A"), textLayer("b", "B")];
    const withHidden = [
      baseImage("a", "A"),
      textLayer("b", "B", { visible: false }),
      textLayer("c", "C"),
    ];
    expect(canMergeVisibleLayers(single)).toBe(false);
    expect(canMergeVisibleLayers(two)).toBe(true);
    expect(canMergeVisibleLayers(withHidden)).toBe(true); // two visible
    expect(canFlattenLayers(single, null)).toBe(false);
    expect(canFlattenLayers(single, "#fff")).toBe(true);
    expect(canFlattenLayers(two, null)).toBe(true);
  });

  it("merges the selected layer into the one below", () => {
    const document = {
      width: 200,
      height: 100,
      background: "#111",
      elements: [
        baseImage("bg", "Background"),
        textLayer("t1", "Hello"),
        textLayer("t2", "World"),
      ],
    };
    const merged = baseImage("merged", "Merged");
    const next = applyMergeLayerDown(document, "t2", merged);
    expect(next.elements.map((element) => element.id)).toEqual(["bg", "merged"]);
    expect(next.background).toBe("#111");
  });

  it("merges only visible layers and keeps hidden ones", () => {
    const document = {
      width: 200,
      height: 100,
      background: null,
      elements: [
        baseImage("bg", "Background"),
        textLayer("hidden", "Secret", { visible: false }),
        textLayer("top", "Hi"),
      ],
    };
    const merged = baseImage("merged", "Merged");
    const next = applyMergeVisibleLayers(document, merged);
    expect(next.elements.map((element) => element.id)).toEqual(["merged", "hidden"]);
  });

  it("flattens into a single locked background layer and clears the canvas fill", () => {
    const document = {
      width: 200,
      height: 100,
      background: "#f7f7f5",
      elements: [
        baseImage("bg", "Background"),
        textLayer("t1", "Hello"),
        textLayer("hidden", "Secret", { visible: false }),
      ],
    };
    const merged = baseImage("flat", "Flattened", { locked: true, source: "background" });
    const next = applyFlattenLayers(document, merged);
    expect(next.background).toBeNull();
    expect(next.elements).toHaveLength(1);
    expect(next.elements[0]).toMatchObject({
      id: "flat",
      locked: true,
      source: "background",
      name: "Flattened",
    });
  });

  it("names merged layers from the first image when present", () => {
    expect(mergedLayerName([textLayer("t", "Hi"), baseImage("i", "Photo")])).toBe("Photo");
    expect(mergedLayerName([textLayer("t", "Hi")])).toBe("Merged");
  });
});

describe("layer preview transform", () => {
  it("fits content bounds into the layers thumbnail with padding and centering", () => {
    const bounds = { x: 100, y: 50, width: 200, height: 100 };
    const { scale, translateX, translateY } = previewTransformForBounds(bounds);

    // 46×34 box, 3px padding → 40×28 inner; 200×100 is width-limited (40/200).
    expect(scale).toBeCloseTo(40 / 200, 5);

    const left = translateX + bounds.x * scale;
    const top = translateY + bounds.y * scale;
    const right = left + bounds.width * scale;
    const bottom = top + bounds.height * scale;

    expect(left).toBeGreaterThanOrEqual(3 - 0.01);
    expect(top).toBeGreaterThanOrEqual(3 - 0.01);
    expect(right).toBeLessThanOrEqual(LAYER_PREVIEW_SIZE.width - 3 + 0.01);
    expect(bottom).toBeLessThanOrEqual(LAYER_PREVIEW_SIZE.height - 3 + 0.01);

    // Flush on the horizontal padding edges; centered vertically.
    expect(left).toBeCloseTo(3, 5);
    expect(right).toBeCloseTo(LAYER_PREVIEW_SIZE.width - 3, 5);
    const centerY = (top + bottom) / 2;
    expect(centerY).toBeCloseTo(LAYER_PREVIEW_SIZE.height / 2, 5);
  });

  it("handles tall thin bounds without exceeding the preview box", () => {
    const bounds = { x: 0, y: 0, width: 4, height: 400 };
    const { scale, translateX, translateY } = previewTransformForBounds(bounds);
    expect(scale).toBeCloseTo(28 / 400, 5);

    const left = translateX + bounds.x * scale;
    const right = left + bounds.width * scale;
    expect(right - left).toBeLessThan(LAYER_PREVIEW_SIZE.width);
    expect(translateY + bounds.y * scale).toBeCloseTo(3, 5);
  });
});
