export type EditorPoint = {
  x: number;
  y: number;
};

export type EditorRect = EditorPoint & {
  width: number;
  height: number;
};

export type ScreenshotTool =
  | "select"
  | "crop"
  | "text"
  | "rectangle"
  | "ellipse"
  | "line"
  | "arrow"
  | "pen";

export type ShapeKind = "rectangle" | "ellipse" | "line" | "arrow";

export type ElementStyle = {
  color: string;
  fill: string | null;
  strokeWidth: number;
};

export type LayerBlendMode =
  | "source-over"
  | "multiply"
  | "screen"
  | "overlay"
  | "darken"
  | "lighten";

export type ImageSnapEdge = "top" | "right" | "bottom" | "left";

/**
 * Where a dropped image lands relative to the snap target layer.
 * Edge values place the import flush against that side; `stack` centers it
 * on top of the target (higher z-order via append-to-layers).
 */
export type ImageDropPlacement = ImageSnapEdge | "stack";

/**
 * Outer fraction of the target layer treated as edge snap zones.
 * Interior (the remaining center) stacks the import on top.
 */
export const IMAGE_DROP_EDGE_BAND_FRACTION = 0.22;

/** Live drop-guide returned while dragging an image over the canvas. */
export type ImageDropGuideInfo = {
  edge: ImageDropPlacement;
  target: EditorRect;
  /** Document-space pointer sample used for stack-light focus tracking. */
  point: EditorPoint;
  /**
   * Estimated native drag-preview footprint used as the stack-light emitter.
   * It follows the pointer without drawing a synthetic preview rectangle.
   */
  focus: EditorRect;
};

type EditorElementBase = {
  id: string;
  x: number;
  y: number;
  locked: boolean;
  visible: boolean;
  opacity: number;
  blendMode: LayerBlendMode;
};

export type EditorImageElement = EditorElementBase & {
  kind: "image";
  source: "background" | "imported";
  src: string;
  name: string;
  /**
   * Capture History / mini-preview artifact this layer came from, when known.
   * Used so the preview stack can show which cards are already in an editor.
   */
  sourceArtifactId: string | null;
  width: number;
  height: number;
  naturalWidth: number;
  naturalHeight: number;
};

export type EditorTextElement = EditorElementBase & {
  kind: "text";
  text: string;
  fontSize: number;
  /**
   * Layout box width used for wrapping. Height is derived from wrapped lines
   * so the selection box tracks content rather than a free-form tall empty area.
   */
  width: number;
  fontFamily: "sans" | "serif" | "mono";
  bold: boolean;
  italic: boolean;
  align: "left" | "center" | "right";
  color: string;
  background: string | null;
};

/** Line box height as a multiple of fontSize (matches canvas + inline editor). */
export const TEXT_LINE_HEIGHT_RATIO = 1.25;

/**
 * Approximate average glyph advance relative to fontSize for sans-like faces.
 * Used when a Canvas measureText context is not available (hit tests, bounds).
 */
export const TEXT_CHAR_WIDTH_RATIO = 0.56;

/** Minimum text box width so a single glyph still fits. */
export function minTextBoxWidth(fontSize: number): number {
  return Math.max(8, Math.round(fontSize * 0.5));
}

/** Estimate the advance width of a single line without a canvas context. */
export function estimateTextWidth(text: string, fontSize: number): number {
  if (!text) return fontSize * TEXT_CHAR_WIDTH_RATIO;
  return Math.max(1, text.length) * fontSize * TEXT_CHAR_WIDTH_RATIO;
}

/** Default box width for newly placed text (fits a short sample with room to grow). */
export function defaultTextBoxWidth(fontSize: number, sample = "Text"): number {
  return Math.max(
    Math.round(fontSize * 2.5),
    Math.ceil(estimateTextWidth(sample, fontSize) + fontSize * 0.75),
  );
}

function hardBreakToken(
  token: string,
  maxWidth: number,
  measure: (line: string) => number,
): string[] {
  const parts: string[] = [];
  let current = "";
  for (const char of [...token]) {
    const next = current + char;
    if (current && measure(next) > maxWidth) {
      parts.push(current);
      current = char;
    } else {
      current = next;
    }
  }
  if (current) parts.push(current);
  return parts.length > 0 ? parts : [token];
}

/**
 * Word-wrap `text` into lines that fit within `maxWidth`.
 * Explicit newlines always break. Tokens wider than the box are hard-broken.
 */
export function wrapTextLines(
  text: string,
  maxWidth: number,
  fontSize: number,
  measure: (line: string) => number = (line) => estimateTextWidth(line, fontSize),
): string[] {
  const width = Math.max(minTextBoxWidth(fontSize), maxWidth);
  const paragraphs = text.split("\n");
  const lines: string[] = [];

  for (const paragraph of paragraphs) {
    if (paragraph === "") {
      lines.push("");
      continue;
    }

    // Keep whitespace tokens so spacing inside a line is preserved.
    const tokens = paragraph.split(/(\s+)/);
    let current = "";

    for (const token of tokens) {
      if (!token) continue;

      if (/^\s+$/.test(token)) {
        if (current) current += token;
        continue;
      }

      const pieces = measure(token) <= width
        ? [token]
        : hardBreakToken(token, width, measure);

      for (const piece of pieces) {
        const candidate = current ? `${current}${piece}` : piece;
        if (current && measure(candidate) > width) {
          lines.push(current.replace(/\s+$/, ""));
          current = piece;
        } else {
          current = candidate;
        }
      }
    }

    lines.push(current.replace(/\s+$/, ""));
  }

  return lines.length > 0 ? lines : [""];
}

export type EditorShapeElement = EditorElementBase & {
  kind: "shape";
  shape: ShapeKind;
  endX: number;
  endY: number;
  /**
   * Intermediate free control points between start `(x, y)` and end `(endX, endY)`.
   * Meaningful for arrows only (other shapes keep this empty).
   * Empty = straight shaft; one point = quadratic curve; more = smooth multi-segment path.
   */
  controls: EditorPoint[];
  style: ElementStyle;
};

/** Maximum intermediate control points on a single arrow. */
export const MAX_ARROW_CONTROLS = 4;

/** Editable handle on a selected arrow (endpoints, free controls, or the default mid). */
export type ArrowHandle =
  | { kind: "start" }
  | { kind: "end" }
  | { kind: "control"; index: number }
  /** Shown only when `controls` is empty — dragging creates the first control. */
  | { kind: "mid" };

export type EditorPathElement = EditorElementBase & {
  kind: "path";
  points: EditorPoint[];
  style: ElementStyle;
};

export type ScreenshotElement =
  | EditorImageElement
  | EditorTextElement
  | EditorShapeElement
  | EditorPathElement;

export type ScreenshotDocument = {
  width: number;
  height: number;
  /** Solid fill behind all layers. `null` keeps the canvas transparent (PNG/WebP alpha). */
  background: string | null;
  elements: ScreenshotElement[];
};

export type LayerDropPlacement = "before" | "after";

const SUPPORTED_IMAGE_EXTENSIONS = new Set([
  "avif",
  "bmp",
  "gif",
  "jpeg",
  "jpg",
  "png",
  "svg",
  "webp",
]);

export function createScreenshotDocument(
  src: string,
  width: number,
  height: number,
  sourceArtifactId: string | null = null,
): ScreenshotDocument {
  return {
    width: Math.max(1, Math.round(width)),
    height: Math.max(1, Math.round(height)),
    background: "#f7f7f5",
    elements: [{
      id: "capture-background",
      kind: "image",
      source: "background",
      src,
      name: "Original screenshot",
      sourceArtifactId,
      x: 0,
      y: 0,
      width,
      height,
      naturalWidth: width,
      naturalHeight: height,
      locked: true,
      visible: true,
      opacity: 100,
      blendMode: "source-over",
    }],
  };
}

/** Artifact IDs currently represented by image layers in a document. */
export function collectEditorSourceArtifactIds(
  elements: readonly ScreenshotElement[],
): string[] {
  const ids = new Set<string>();
  for (const element of elements) {
    if (element.kind === "image" && element.sourceArtifactId) {
      ids.add(element.sourceArtifactId);
    }
  }
  return [...ids].sort();
}

export function normalizeRect(start: EditorPoint, end: EditorPoint): EditorRect {
  const x = Math.min(start.x, end.x);
  const y = Math.min(start.y, end.y);
  return {
    x,
    y,
    width: Math.abs(end.x - start.x),
    height: Math.abs(end.y - start.y),
  };
}

export function boundedCropRect(
  start: EditorPoint,
  end: EditorPoint,
  bounds: Pick<ScreenshotDocument, "width" | "height">,
  aspectRatio: number | null = null,
): EditorRect {
  const boundedStart = {
    x: clamp(start.x, 0, bounds.width),
    y: clamp(start.y, 0, bounds.height),
  };
  let boundedEnd = {
    x: clamp(end.x, 0, bounds.width),
    y: clamp(end.y, 0, bounds.height),
  };

  if (aspectRatio && Number.isFinite(aspectRatio) && aspectRatio > 0) {
    const directionX = boundedEnd.x < boundedStart.x ? -1 : 1;
    const directionY = boundedEnd.y < boundedStart.y ? -1 : 1;
    let width = Math.abs(boundedEnd.x - boundedStart.x);
    let height = Math.abs(boundedEnd.y - boundedStart.y);
    if (height === 0 || width / height > aspectRatio) {
      height = width / aspectRatio;
    } else {
      width = height * aspectRatio;
    }
    width = Math.min(width, directionX > 0 ? bounds.width - boundedStart.x : boundedStart.x);
    height = width / aspectRatio;
    if (height > (directionY > 0 ? bounds.height - boundedStart.y : boundedStart.y)) {
      height = directionY > 0 ? bounds.height - boundedStart.y : boundedStart.y;
      width = height * aspectRatio;
    }
    boundedEnd = {
      x: boundedStart.x + width * directionX,
      y: boundedStart.y + height * directionY,
    };
  }

  const rect = normalizeRect(boundedStart, boundedEnd);
  return {
    x: Math.round(rect.x),
    y: Math.round(rect.y),
    width: Math.max(1, Math.round(rect.width)),
    height: Math.max(1, Math.round(rect.height)),
  };
}

export function cropDocument(
  document: ScreenshotDocument,
  crop: EditorRect,
): ScreenshotDocument {
  const bounded = {
    x: clamp(Math.round(crop.x), 0, Math.max(0, document.width - 1)),
    y: clamp(Math.round(crop.y), 0, Math.max(0, document.height - 1)),
    width: clamp(Math.round(crop.width), 1, document.width),
    height: clamp(Math.round(crop.height), 1, document.height),
  };
  bounded.width = Math.min(bounded.width, document.width - bounded.x);
  bounded.height = Math.min(bounded.height, document.height - bounded.y);
  return {
    ...document,
    width: bounded.width,
    height: bounded.height,
    elements: document.elements.map((element) => translateElement(
      element,
      -bounded.x,
      -bounded.y,
    )),
  };
}

/**
 * Axis-aligned union of every *visible* layer's bounds (including locked ones).
 * Hidden layers are ignored so they do not hold empty canvas open.
 * Returns `null` when nothing is visible.
 */
export function visibleContentBounds(
  document: Pick<ScreenshotDocument, "elements">,
): EditorRect | null {
  let minX = Number.POSITIVE_INFINITY;
  let minY = Number.POSITIVE_INFINITY;
  let maxX = Number.NEGATIVE_INFINITY;
  let maxY = Number.NEGATIVE_INFINITY;
  let found = false;
  for (const element of document.elements) {
    if (!element.visible) continue;
    found = true;
    const bounds = elementBounds(element);
    minX = Math.min(minX, bounds.x);
    minY = Math.min(minY, bounds.y);
    maxX = Math.max(maxX, bounds.x + bounds.width);
    maxY = Math.max(maxY, bounds.y + bounds.height);
  }
  if (!found) return null;
  return {
    x: minX,
    y: minY,
    width: Math.max(1, maxX - minX),
    height: Math.max(1, maxY - minY),
  };
}

/**
 * Shrink (or expand-then-fit) the canvas so empty margin around visible layers
 * is removed — like Sharp's `trim()`, but driven by layer geometry rather than
 * flattened pixel sampling. Optional padding keeps a uniform border.
 *
 * Content outside the current canvas is kept and becomes the new frame origin.
 * Returns the same document reference when already tight or nothing is visible.
 */
export function trimDocumentToContent(
  document: ScreenshotDocument,
  padding = 0,
): ScreenshotDocument {
  const content = visibleContentBounds(document);
  if (!content) return document;

  const safePadding = Math.max(0, Math.round(padding));
  const x = Math.floor(content.x) - safePadding;
  const y = Math.floor(content.y) - safePadding;
  const right = Math.ceil(content.x + content.width) + safePadding;
  const bottom = Math.ceil(content.y + content.height) + safePadding;
  const width = Math.max(1, right - x);
  const height = Math.max(1, bottom - y);

  if (
    x === 0
    && y === 0
    && width === document.width
    && height === document.height
  ) {
    return document;
  }

  return {
    ...document,
    width,
    height,
    elements: document.elements.map((element) => translateElement(element, -x, -y)),
  };
}

export function resizeDocumentCanvas(
  document: ScreenshotDocument,
  width: number,
  height: number,
): ScreenshotDocument {
  return {
    ...document,
    width: clamp(Math.round(width), 1, 32_768),
    height: clamp(Math.round(height), 1, 32_768),
  };
}

export function positionImportedImage(
  naturalWidth: number,
  naturalHeight: number,
  document: Pick<ScreenshotDocument, "width" | "height">,
  dropPoint?: EditorPoint,
): EditorRect {
  const safeWidth = Math.max(1, naturalWidth);
  const safeHeight = Math.max(1, naturalHeight);
  const maximumWidth = Math.max(160, document.width * 0.65);
  const maximumHeight = Math.max(120, document.height * 0.65);
  const scale = Math.min(1, maximumWidth / safeWidth, maximumHeight / safeHeight);
  const width = Math.max(1, Math.round(safeWidth * scale));
  const height = Math.max(1, Math.round(safeHeight * scale));
  const center = dropPoint ?? {
    x: document.width / 2,
    y: document.height / 2,
  };
  return {
    x: Math.max(0, Math.round(center.x - width / 2)),
    y: Math.max(0, Math.round(center.y - height / 2)),
    width,
    height,
  };
}

export function closestImageSnapEdge(
  point: EditorPoint,
  target: EditorRect,
): ImageSnapEdge {
  const distances: Array<[ImageSnapEdge, number]> = [
    ["top", Math.abs(point.y - target.y)],
    ["right", Math.abs(point.x - (target.x + target.width))],
    ["bottom", Math.abs(point.y - (target.y + target.height))],
    ["left", Math.abs(point.x - target.x)],
  ];
  distances.sort((left, right) => left[1] - right[1]);
  return distances[0][0];
}

/**
 * Pick edge snap vs stack-on-top for a pointer over a drop target.
 * Interior of the target (outside the outer edge bands) stacks; near an edge
 * or outside the rect still snaps to the closest edge.
 */
export function imageDropPlacementAtPoint(
  point: EditorPoint,
  target: EditorRect,
): ImageDropPlacement {
  const relativeX = point.x - target.x;
  const relativeY = point.y - target.y;
  const inside = relativeX >= 0
    && relativeY >= 0
    && relativeX <= target.width
    && relativeY <= target.height;
  if (inside && target.width > 0 && target.height > 0) {
    const edgeBandX = target.width * IMAGE_DROP_EDGE_BAND_FRACTION;
    const edgeBandY = target.height * IMAGE_DROP_EDGE_BAND_FRACTION;
    // Only offer a real stack zone when both axes leave some interior room.
    if (edgeBandX * 2 < target.width && edgeBandY * 2 < target.height) {
      const nearLeft = relativeX < edgeBandX;
      const nearRight = relativeX > target.width - edgeBandX;
      const nearTop = relativeY < edgeBandY;
      const nearBottom = relativeY > target.height - edgeBandY;
      if (!nearLeft && !nearRight && !nearTop && !nearBottom) {
        return "stack";
      }
    }
  }
  return closestImageSnapEdge(point, target);
}

/**
 * Pick the layer bounds that an imported image should snap against.
 * Prefer the selected visible image; otherwise the front-most visible image;
 * otherwise the full canvas.
 */
export function resolveImageDropTarget(
  document: Pick<ScreenshotDocument, "width" | "height" | "elements">,
  selectedId: string | null,
): EditorRect {
  const selected = document.elements.find((element) => (
    element.id === selectedId
    && element.kind === "image"
    && element.visible
  ));
  if (selected) return elementBounds(selected);

  for (let index = document.elements.length - 1; index >= 0; index -= 1) {
    const element = document.elements[index];
    if (element.kind === "image" && element.visible) {
      return elementBounds(element);
    }
  }

  return { x: 0, y: 0, width: document.width, height: document.height };
}

/**
 * Estimate the native drag-preview footprint under the pointer. It stays
 * smaller than the target so the light remains local to the floating preview.
 */
export function stackDropLightFocusAtPoint(
  point: EditorPoint,
  target: EditorRect,
): EditorRect {
  const shortSide = Math.max(1, Math.min(target.width, target.height));
  const width = Math.max(
    72,
    Math.min(shortSide * 0.32, target.width * 0.36, 260),
  );
  const height = Math.max(
    54,
    Math.min(width * 0.78, target.height * 0.36, 200),
  );
  const rawX = point.x - width / 2;
  const rawY = point.y - height / 2;
  // Allow a little overhang so the preview emitter stays on the pointer near
  // edges. When it is larger than the target, center instead of inverting clamp.
  const minX = target.x - width * 0.2;
  const maxX = target.x + target.width - width * 0.8;
  const minY = target.y - height * 0.2;
  const maxY = target.y + target.height - height * 0.8;
  const x = minX <= maxX
    ? clamp(rawX, minX, maxX)
    : target.x + (target.width - width) / 2;
  const y = minY <= maxY
    ? clamp(rawY, minY, maxY)
    : target.y + (target.height - height) / 2;
  return { x, y, width, height };
}

/** Live drop-guide for an image import at a document-space pointer position. */
export function imageDropGuideAtPoint(
  document: Pick<ScreenshotDocument, "width" | "height" | "elements">,
  selectedId: string | null,
  point: EditorPoint,
): ImageDropGuideInfo {
  const target = resolveImageDropTarget(document, selectedId);
  return {
    edge: imageDropPlacementAtPoint(point, target),
    target,
    point,
    focus: stackDropLightFocusAtPoint(point, target),
  };
}

/**
 * Position an imported image relative to a drop target: flush to an edge, or
 * centered on the pointer when stacking on top (`stack` + optional `point`).
 */
export function positionImportedImageAtEdge(
  naturalWidth: number,
  naturalHeight: number,
  document: Pick<ScreenshotDocument, "width" | "height">,
  target: EditorRect,
  edge: ImageDropPlacement,
  point?: EditorPoint,
): EditorRect {
  const stackCenter = point ?? {
    x: target.x + target.width / 2,
    y: target.y + target.height / 2,
  };
  const centered = positionImportedImage(
    naturalWidth,
    naturalHeight,
    document,
    edge === "stack"
      ? stackCenter
      : { x: target.x + target.width / 2, y: target.y + target.height / 2 },
  );
  if (edge === "stack") {
    return {
      ...centered,
      x: Math.round(stackCenter.x - centered.width / 2),
      y: Math.round(stackCenter.y - centered.height / 2),
    };
  }
  if (edge === "top") {
    return {
      ...centered,
      x: Math.round(target.x + (target.width - centered.width) / 2),
      y: Math.round(target.y - centered.height),
    };
  }
  if (edge === "right") {
    return {
      ...centered,
      x: Math.round(target.x + target.width),
      y: Math.round(target.y + (target.height - centered.height) / 2),
    };
  }
  if (edge === "left") {
    return {
      ...centered,
      x: Math.round(target.x - centered.width),
      y: Math.round(target.y + (target.height - centered.height) / 2),
    };
  }
  return {
    ...centered,
    x: Math.round(target.x + (target.width - centered.width) / 2),
    y: Math.round(target.y + target.height),
  };
}

export function expandDocumentForElement(
  document: ScreenshotDocument,
  element: ScreenshotElement,
  padding = 24,
): ScreenshotDocument {
  const bounds = elementBounds(element);
  const shiftX = Math.max(0, Math.ceil(-bounds.x));
  const shiftY = Math.max(0, Math.ceil(-bounds.y));
  const shiftedElement = translateElement(element, shiftX, shiftY);
  const shiftedBounds = elementBounds(shiftedElement);
  return {
    ...document,
    width: Math.max(
      document.width + shiftX,
      Math.ceil(shiftedBounds.x + shiftedBounds.width + padding),
    ),
    height: Math.max(
      document.height + shiftY,
      Math.ceil(shiftedBounds.y + shiftedBounds.height + padding),
    ),
    elements: [
      ...document.elements.map((current) => translateElement(current, shiftX, shiftY)),
      shiftedElement,
    ],
  };
}

/**
 * Preview of the expanded canvas in *current* document coordinates (origin still
 * at the pre-expand top-left). Null when no growth is needed. Matches
 * {@link expandDocumentToFitBounds} geometry without mutating layers.
 */
export function previewExpandedCanvasRect(
  bounds: EditorRect,
  canvas: Pick<ScreenshotDocument, "width" | "height">,
  padding = 0,
): EditorRect | null {
  const shiftX = Math.max(0, Math.ceil(-bounds.x));
  const shiftY = Math.max(0, Math.ceil(-bounds.y));
  const fittedX = bounds.x + shiftX;
  const fittedY = bounds.y + shiftY;
  const width = Math.max(
    canvas.width + shiftX,
    Math.ceil(fittedX + bounds.width + padding),
  );
  const height = Math.max(
    canvas.height + shiftY,
    Math.ceil(fittedY + bounds.height + padding),
  );
  if (
    shiftX === 0
    && shiftY === 0
    && width === canvas.width
    && height === canvas.height
  ) {
    return null;
  }
  return {
    // Normalize -0 to 0 so previews match exact object equality in tests/UI.
    x: shiftX === 0 ? 0 : -shiftX,
    y: shiftY === 0 ? 0 : -shiftY,
    width,
    height,
  };
}

/**
 * Grow the canvas so `bounds` sits fully inside it. Negative overflow shifts every
 * layer so content stays put relative to the expanded frame.
 */
export function expandDocumentToFitBounds(
  document: ScreenshotDocument,
  bounds: EditorRect,
  padding = 0,
): ScreenshotDocument {
  const preview = previewExpandedCanvasRect(bounds, document, padding);
  if (!preview) return document;
  const shiftX = -preview.x;
  const shiftY = -preview.y;
  return {
    ...document,
    width: preview.width,
    height: preview.height,
    elements: document.elements.map((current) => translateElement(current, shiftX, shiftY)),
  };
}

/** Screen-pixel snap distance converted to document units via display scale. */
export const ALIGNMENT_SNAP_SCREEN_PX = 10;

export type AlignmentSnapGuide = {
  orientation: "vertical" | "horizontal";
  /** Document X for vertical guides, Y for horizontal. */
  position: number;
};

export type BoundsSnapResult = {
  bounds: EditorRect;
  guides: AlignmentSnapGuide[];
};

/**
 * Vertical/horizontal lines layers can snap against: canvas borders plus every
 * other visible layer's edges.
 */
export function collectAlignmentSnapLines(
  document: Pick<ScreenshotDocument, "width" | "height" | "elements">,
  excludeId: string | null,
): { vertical: number[]; horizontal: number[] } {
  const vertical = new Set<number>([0, document.width]);
  const horizontal = new Set<number>([0, document.height]);
  for (const element of document.elements) {
    if (element.id === excludeId || !element.visible) continue;
    const bounds = elementBounds(element);
    vertical.add(bounds.x);
    vertical.add(bounds.x + bounds.width);
    horizontal.add(bounds.y);
    horizontal.add(bounds.y + bounds.height);
  }
  return {
    vertical: [...vertical],
    horizontal: [...horizontal],
  };
}

function closestSnapPosition(
  value: number,
  lines: number[],
  threshold: number,
): number | null {
  let best: number | null = null;
  let bestDistance = threshold;
  for (const line of lines) {
    const distance = Math.abs(line - value);
    if (distance <= bestDistance) {
      bestDistance = distance;
      best = line;
    }
  }
  return best;
}

/**
 * Snap a freely translated box so its edges align with nearby snap lines.
 * Picks at most one X delta and one Y delta (closest within threshold).
 */
export function snapTranslatedBounds(
  bounds: EditorRect,
  lines: { vertical: number[]; horizontal: number[] },
  threshold: number,
): BoundsSnapResult {
  if (threshold <= 0) return { bounds, guides: [] };

  type AxisHit = { delta: number; position: number };
  const pickAxis = (
    candidates: number[],
    snapLines: number[],
  ): AxisHit | null => {
    let best: AxisHit | null = null;
    let bestAbs = threshold;
    for (const value of candidates) {
      const snapped = closestSnapPosition(value, snapLines, threshold);
      if (snapped === null) continue;
      const delta = snapped - value;
      const abs = Math.abs(delta);
      if (abs < bestAbs - 1e-9) {
        bestAbs = abs;
        best = { delta, position: snapped };
      }
    }
    return best;
  };

  const left = bounds.x;
  const right = bounds.x + bounds.width;
  const top = bounds.y;
  const bottom = bounds.y + bounds.height;
  const xHit = pickAxis([left, right], lines.vertical);
  const yHit = pickAxis([top, bottom], lines.horizontal);

  const next = {
    ...bounds,
    x: bounds.x + (xHit?.delta ?? 0),
    y: bounds.y + (yHit?.delta ?? 0),
  };
  const guides: AlignmentSnapGuide[] = [];
  const pushUnique = (guide: AlignmentSnapGuide) => {
    if (guides.some((existing) => (
      existing.orientation === guide.orientation
      && Math.abs(existing.position - guide.position) <= 1e-6
    ))) {
      return;
    }
    guides.push(guide);
  };
  if (xHit) {
    // Light every edge that now sits on a snap line (same-width dual matches).
    for (const edge of [next.x, next.x + next.width]) {
      if (lines.vertical.some((line) => Math.abs(line - edge) <= 1e-6)) {
        pushUnique({ orientation: "vertical", position: edge });
      }
    }
  }
  if (yHit) {
    for (const edge of [next.y, next.y + next.height]) {
      if (lines.horizontal.some((line) => Math.abs(line - edge) <= 1e-6)) {
        pushUnique({ orientation: "horizontal", position: edge });
      }
    }
  }
  return { bounds: next, guides };
}

/**
 * Snap edges moved by a corner resize so they align with nearby snap lines.
 * The corner opposite the handle (before any flip) stays fixed when possible.
 */
export function snapResizedBounds(
  initialBounds: EditorRect,
  handle: ResizeHandle,
  nextBounds: EditorRect,
  lines: { vertical: number[]; horizontal: number[] },
  threshold: number,
  minimumSize = 8,
): BoundsSnapResult {
  if (threshold <= 0) return { bounds: nextBounds, guides: [] };
  const min = Math.max(1, minimumSize);
  const anchor = resizeHandlePoint(initialBounds, oppositeResizeHandle(handle));
  let { x, y, width, height } = nextBounds;
  const guides: AlignmentSnapGuide[] = [];

  const leftFree = Math.abs(x - anchor.x) > 0.5;
  const rightFree = Math.abs(x + width - anchor.x) > 0.5;
  const topFree = Math.abs(y - anchor.y) > 0.5;
  const bottomFree = Math.abs(y + height - anchor.y) > 0.5;

  if (leftFree) {
    const snapped = closestSnapPosition(x, lines.vertical, threshold);
    if (snapped !== null) {
      const nextWidth = width + (x - snapped);
      if (nextWidth >= min) {
        width = nextWidth;
        x = snapped;
        guides.push({ orientation: "vertical", position: snapped });
      }
    }
  }
  if (rightFree) {
    const right = x + width;
    const snapped = closestSnapPosition(right, lines.vertical, threshold);
    if (snapped !== null) {
      const nextWidth = snapped - x;
      if (nextWidth >= min) {
        width = nextWidth;
        guides.push({ orientation: "vertical", position: snapped });
      }
    }
  }
  if (topFree) {
    const snapped = closestSnapPosition(y, lines.horizontal, threshold);
    if (snapped !== null) {
      const nextHeight = height + (y - snapped);
      if (nextHeight >= min) {
        height = nextHeight;
        y = snapped;
        guides.push({ orientation: "horizontal", position: snapped });
      }
    }
  }
  if (bottomFree) {
    const bottom = y + height;
    const snapped = closestSnapPosition(bottom, lines.horizontal, threshold);
    if (snapped !== null) {
      const nextHeight = snapped - y;
      if (nextHeight >= min) {
        height = nextHeight;
        guides.push({ orientation: "horizontal", position: snapped });
      }
    }
  }

  return {
    bounds: { x, y, width, height },
    guides,
  };
}

/** Which canvas borders a box crosses (partially outside the document). */
export function canvasOverflowEdges(
  bounds: EditorRect,
  canvas: Pick<ScreenshotDocument, "width" | "height">,
  epsilon = 0.5,
): ImageSnapEdge[] {
  const edges: ImageSnapEdge[] = [];
  if (bounds.x < -epsilon) edges.push("left");
  if (bounds.y < -epsilon) edges.push("top");
  if (bounds.x + bounds.width > canvas.width + epsilon) edges.push("right");
  if (bounds.y + bounds.height > canvas.height + epsilon) edges.push("bottom");
  return edges;
}

export function isSupportedImageFile(file: Pick<File, "name" | "type">): boolean {
  if (file.type.toLowerCase().startsWith("image/")) return true;
  const extension = file.name.split(".").at(-1)?.toLowerCase() ?? "";
  return SUPPORTED_IMAGE_EXTENSIONS.has(extension);
}

/**
 * Screenshot elements are stored back-to-front, while the layer panel is
 * presented front-to-back. Reorder in the panel's visual order, then convert
 * back. Locked layers remain fixed until the user explicitly unlocks them.
 */
export function reorderScreenshotLayers(
  elements: ScreenshotElement[],
  movedId: string,
  targetId: string,
  placement: LayerDropPlacement,
): ScreenshotElement[] {
  const moved = elements.find((element) => element.id === movedId);
  if (
    !moved
    || movedId === targetId
    || moved.locked
  ) {
    return elements;
  }

  const movedIndex = elements.findIndex((element) => element.id === movedId);
  const remaining = elements.filter((element) => element.id !== movedId);
  const targetIndex = remaining.findIndex((element) => element.id === targetId);
  if (targetIndex < 0) return elements;

  // In storage order, panel "before" means immediately in front of the
  // target. Clamp the insertion to the unlocked run that originally contained
  // the layer so every locked layer keeps its exact stack position.
  const desired = targetIndex + (placement === "before" ? 1 : 0);
  const lockedBelow = elements
    .map((element, index) => ({ element, index }))
    .filter(({ element, index }) => element.locked && index < movedIndex)
    .at(-1)?.index;
  const lockedAbove = elements.findIndex(
    (element, index) => element.locked && index > movedIndex,
  );
  const minimum = lockedBelow === undefined ? 0 : lockedBelow + 1;
  const maximum = lockedAbove < 0 ? remaining.length : lockedAbove - 1;
  const destination = clamp(desired, minimum, maximum);
  remaining.splice(destination, 0, moved);
  return remaining;
}

export function duplicateScreenshotElement(
  element: ScreenshotElement,
  id: string,
  offset = 24,
): ScreenshotElement {
  const copy = translateElement({
    ...element,
    id,
    locked: false,
    visible: true,
    ...(element.kind === "image"
      ? {
        source: "imported" as const,
        name: `${element.name} copy`,
      }
      : {}),
  }, offset, offset);
  return copy;
}

export function translateElement(
  element: ScreenshotElement,
  deltaX: number,
  deltaY: number,
): ScreenshotElement {
  if (element.kind === "path") {
    return {
      ...element,
      x: element.x + deltaX,
      y: element.y + deltaY,
      points: element.points.map((point) => ({
        x: point.x + deltaX,
        y: point.y + deltaY,
      })),
    };
  }
  if (element.kind === "shape") {
    return {
      ...element,
      x: element.x + deltaX,
      y: element.y + deltaY,
      endX: element.endX + deltaX,
      endY: element.endY + deltaY,
      controls: element.controls.map((point) => ({
        x: point.x + deltaX,
        y: point.y + deltaY,
      })),
    };
  }
  return {
    ...element,
    x: element.x + deltaX,
    y: element.y + deltaY,
  };
}

/** Ordered vertices for an arrow: start → free controls → end. */
export function arrowVertices(element: EditorShapeElement): EditorPoint[] {
  return [
    { x: element.x, y: element.y },
    ...element.controls,
    { x: element.endX, y: element.endY },
  ];
}

/** Midpoint of the start→end chord (default mid handle when the arrow is straight). */
export function arrowDefaultMidHandle(element: EditorShapeElement): EditorPoint {
  return {
    x: (element.x + element.endX) / 2,
    y: (element.y + element.endY) / 2,
  };
}

/**
 * Point used for the arrowhead tangent (approximate direction into the tip).
 * For a single free control this is the quadratic control; otherwise the
 * second-to-last vertex.
 */
export function arrowHeadTangentPoint(element: EditorShapeElement): EditorPoint {
  if (element.controls.length === 1) {
    return element.controls[0];
  }
  if (element.controls.length > 1) {
    return element.controls[element.controls.length - 1];
  }
  return { x: element.x, y: element.y };
}

/**
 * Sample points along the arrow shaft for hit-testing and closest-point queries.
 * Matches the canvas stroke: straight, single quadratic, or smooth multi-segment.
 */
export function sampleArrowPath(
  element: EditorShapeElement,
  samplesPerSegment = 24,
): EditorPoint[] {
  const vertices = arrowVertices(element);
  if (vertices.length < 2) return vertices;
  const samples: EditorPoint[] = [];
  const steps = Math.max(4, samplesPerSegment);

  if (vertices.length === 2) {
    const [p0, p1] = vertices;
    for (let i = 0; i <= steps; i += 1) {
      const t = i / steps;
      samples.push({
        x: p0.x + (p1.x - p0.x) * t,
        y: p0.y + (p1.y - p0.y) * t,
      });
    }
    return samples;
  }

  if (vertices.length === 3) {
    const [p0, p1, p2] = vertices;
    for (let i = 0; i <= steps; i += 1) {
      const t = i / steps;
      const mt = 1 - t;
      samples.push({
        x: mt * mt * p0.x + 2 * mt * t * p1.x + t * t * p2.x,
        y: mt * mt * p0.y + 2 * mt * t * p1.y + t * t * p2.y,
      });
    }
    return samples;
  }

  // Smooth multi-point path (same quadratic midpoint scheme as freehand paths).
  samples.push(vertices[0]);
  for (let index = 1; index < vertices.length - 2; index += 1) {
    const from = samples.at(-1)!;
    const control = vertices[index];
    const to = {
      x: (vertices[index].x + vertices[index + 1].x) / 2,
      y: (vertices[index].y + vertices[index + 1].y) / 2,
    };
    for (let i = 1; i <= steps; i += 1) {
      const t = i / steps;
      const mt = 1 - t;
      samples.push({
        x: mt * mt * from.x + 2 * mt * t * control.x + t * t * to.x,
        y: mt * mt * from.y + 2 * mt * t * control.y + t * t * to.y,
      });
    }
  }
  {
    const from = samples.at(-1)!;
    const control = vertices[vertices.length - 2];
    const to = vertices[vertices.length - 1];
    for (let i = 1; i <= steps; i += 1) {
      const t = i / steps;
      const mt = 1 - t;
      samples.push({
        x: mt * mt * from.x + 2 * mt * t * control.x + t * t * to.x,
        y: mt * mt * from.y + 2 * mt * t * control.y + t * t * to.y,
      });
    }
  }
  return samples;
}

/**
 * Closest sampled point on the arrow path and the control-array index where a
 * new control should be inserted to land near that spot.
 */
export function closestPointOnArrow(
  element: EditorShapeElement,
  point: EditorPoint,
  samplesPerSegment = 24,
): { point: EditorPoint; insertIndex: number; distance: number } {
  const samples = sampleArrowPath(element, samplesPerSegment);
  let best = samples[0] ?? { x: element.x, y: element.y };
  let bestDistance = Number.POSITIVE_INFINITY;
  let bestSampleIndex = 0;
  for (let index = 0; index < samples.length; index += 1) {
    const sample = samples[index];
    const distance = Math.hypot(point.x - sample.x, point.y - sample.y);
    if (distance < bestDistance) {
      bestDistance = distance;
      best = sample;
      bestSampleIndex = index;
    }
  }

  // Map sample progress (0..1) onto the control insert index (0..controls.length).
  const progress = samples.length <= 1
    ? 0.5
    : bestSampleIndex / (samples.length - 1);
  const insertIndex = clamp(
    Math.floor(progress * (element.controls.length + 1)),
    0,
    element.controls.length,
  );

  return {
    point: best,
    insertIndex,
    distance: bestDistance,
  };
}

/**
 * Drawn arrow-head wing length (must match the renderer in ScreenshotEditor).
 * Used for content bounds so trim/expand account for the head, not just the shaft.
 */
export function arrowHeadLength(strokeWidth: number): number {
  return Math.max(14, strokeWidth * 4.2);
}

/** Outward extent of a stroked path (half width + round cap/join slop). */
function strokeExtent(strokeWidth: number): number {
  return Math.max(2, Math.ceil(strokeWidth / 2) + 1);
}

/** Axis-aligned box around points, expanded by uniform padding. */
function boundsFromPoints(points: EditorPoint[], padding: number): EditorRect {
  const xs = points.map(({ x }) => x);
  const ys = points.map(({ y }) => y);
  const left = Math.min(...xs);
  const top = Math.min(...ys);
  const right = Math.max(...xs);
  const bottom = Math.max(...ys);
  const pad = Math.max(0, padding);
  return {
    x: left - pad,
    y: top - pad,
    width: Math.max(1, right - left) + pad * 2,
    height: Math.max(1, bottom - top) + pad * 2,
  };
}

/**
 * Bounds of the painted shape (stroke + arrow head), not the loose selection
 * chrome.
 *
 * Free Bezier control points sit off the stroke (the path only approaches
 * them). Including them in content bounds used to expand the canvas and block
 * **Trim edges** whenever a handle sat past an edge even though the painted
 * shaft stayed inside. Bounds therefore follow path samples for arrows, not
 * the control handles.
 */
function shapeElementBounds(element: EditorShapeElement): EditorRect {
  const strokePad = strokeExtent(element.style.strokeWidth);

  if (element.shape === "rectangle" || element.shape === "ellipse") {
    const rect = normalizeRect(
      { x: element.x, y: element.y },
      { x: element.endX, y: element.endY },
    );
    return {
      x: rect.x - strokePad,
      y: rect.y - strokePad,
      width: Math.max(1, rect.width) + strokePad * 2,
      height: Math.max(1, rect.height) + strokePad * 2,
    };
  }

  if (element.shape === "arrow") {
    // Sample the same quadratic / multi-point path the canvas draws.
    const samples = sampleArrowPath(element, 24);
    const points = samples.length > 0
      ? samples
      : [
        { x: element.x, y: element.y },
        { x: element.endX, y: element.endY },
      ];
    // Wings leave the tip at ±30°; lateral extent ≈ sin(30°) * length.
    const headPad = Math.ceil(arrowHeadLength(element.style.strokeWidth) * 0.55);
    return boundsFromPoints(points, strokePad + headPad);
  }

  // Line: hull of endpoints + stroke extent.
  return boundsFromPoints(
    [
      { x: element.x, y: element.y },
      { x: element.endX, y: element.endY },
    ],
    strokePad,
  );
}

/**
 * Single-control bend amount for the Curve slider (−1…1). Zero when straight
 * or multi-control. Positive/negative curve to opposite sides of the chord.
 */
export function arrowBendAmount(
  element: EditorShapeElement,
  maximumBend = 1,
): number {
  if (element.shape !== "arrow" || element.controls.length !== 1) return 0;
  return arrowBendFromControlPoint(element, element.controls[0], maximumBend);
}

/**
 * Convert a free point into the normalized lateral bend of a single mid control
 * (perpendicular projection onto the start→end chord).
 */
export function arrowBendFromControlPoint(
  element: EditorShapeElement,
  point: EditorPoint,
  maximumBend = 1,
): number {
  if (element.shape !== "arrow") return 0;
  const deltaX = element.endX - element.x;
  const deltaY = element.endY - element.y;
  const lengthSquared = deltaX * deltaX + deltaY * deltaY;
  if (lengthSquared < 1) return 0;
  const midpointX = (element.x + element.endX) / 2;
  const midpointY = (element.y + element.endY) / 2;
  const projected = (
    (point.x - midpointX) * -deltaY
    + (point.y - midpointY) * deltaX
  ) / lengthSquared;
  const limit = Math.max(0, maximumBend);
  return clamp(projected, -limit, limit);
}

/** Build a single mid control from a normalized bend amount (Curve slider). */
export function arrowControlFromBend(
  element: EditorShapeElement,
  bend: number,
): EditorPoint {
  const deltaX = element.endX - element.x;
  const deltaY = element.endY - element.y;
  return {
    x: (element.x + element.endX) / 2 - deltaY * bend,
    y: (element.y + element.endY) / 2 + deltaX * bend,
  };
}

/**
 * Apply Curve-slider bend: empty/one control becomes a pure perpendicular mid
 * control; near-zero bend clears controls back to a straight arrow.
 */
export function arrowWithBend(
  element: EditorShapeElement,
  bend: number,
): EditorShapeElement {
  if (element.shape !== "arrow") return element;
  if (Math.abs(bend) < 0.005) {
    return { ...element, controls: [] };
  }
  return {
    ...element,
    controls: [arrowControlFromBend(element, bend)],
  };
}

/** Insert a free control at `point` (capped). Returns null when at the max. */
export function insertArrowControl(
  element: EditorShapeElement,
  point: EditorPoint,
): EditorShapeElement | null {
  if (element.shape !== "arrow") return null;
  if (element.controls.length >= MAX_ARROW_CONTROLS) return null;
  const { insertIndex } = closestPointOnArrow(element, point);
  const controls = [
    ...element.controls.slice(0, insertIndex),
    { x: point.x, y: point.y },
    ...element.controls.slice(insertIndex),
  ];
  return { ...element, controls };
}

export function removeArrowControl(
  element: EditorShapeElement,
  index: number,
): EditorShapeElement {
  if (element.shape !== "arrow") return element;
  if (index < 0 || index >= element.controls.length) return element;
  return {
    ...element,
    controls: element.controls.filter((_, i) => i !== index),
  };
}

/**
 * Hit-test arrow edit handles. Order: free controls → endpoints → default mid.
 * Returns null when the pointer is not on a handle.
 */
export function hitTestArrowHandle(
  element: EditorShapeElement,
  point: EditorPoint,
  handleRadius: number,
): ArrowHandle | null {
  if (element.shape !== "arrow") return null;
  const radius = Math.max(4, handleRadius);

  for (let index = 0; index < element.controls.length; index += 1) {
    const control = element.controls[index];
    if (Math.hypot(point.x - control.x, point.y - control.y) <= radius) {
      return { kind: "control", index };
    }
  }

  if (Math.hypot(point.x - element.x, point.y - element.y) <= radius) {
    return { kind: "start" };
  }
  if (Math.hypot(point.x - element.endX, point.y - element.endY) <= radius) {
    return { kind: "end" };
  }

  if (element.controls.length === 0) {
    const mid = arrowDefaultMidHandle(element);
    // Slightly larger hit target so the on-path mid handle is easy to grab.
    if (Math.hypot(point.x - mid.x, point.y - mid.y) <= radius * 1.15) {
      return { kind: "mid" };
    }
  }

  return null;
}

/** @deprecated Prefer `hitTestArrowHandle`; kept for call sites that only need mid/control. */
export function hitTestArrowControlPoint(
  element: EditorShapeElement,
  point: EditorPoint,
  handleRadius: number,
): boolean {
  const handle = hitTestArrowHandle(element, point, handleRadius);
  return handle?.kind === "control" || handle?.kind === "mid";
}

/**
 * Legacy single control-point position for one free control, or the default mid
 * when the arrow is straight. Prefer `arrowVertices` / free controls for new UI.
 */
export function arrowControlPoint(element: EditorShapeElement): EditorPoint | null {
  if (element.shape !== "arrow") return null;
  if (element.controls.length === 1) return element.controls[0];
  if (element.controls.length === 0) return arrowDefaultMidHandle(element);
  return null;
}

/** Corner handles drawn around a selected annotation. */
export type ResizeHandle = "nw" | "ne" | "sw" | "se";

const RESIZE_HANDLES: ResizeHandle[] = ["nw", "ne", "se", "sw"];

/**
 * Hit-test the four corner resize handles of a selection box.
 * `handleRadius` is in document pixels (scale for display zoom before calling).
 */
export function hitTestResizeHandle(
  bounds: EditorRect,
  point: EditorPoint,
  handleRadius: number,
): ResizeHandle | null {
  const radius = Math.max(4, handleRadius);
  for (const handle of RESIZE_HANDLES) {
    const corner = resizeHandlePoint(bounds, handle);
    if (
      Math.abs(point.x - corner.x) <= radius
      && Math.abs(point.y - corner.y) <= radius
    ) {
      return handle;
    }
  }
  return null;
}

export function resizeHandlePoint(bounds: EditorRect, handle: ResizeHandle): EditorPoint {
  if (handle === "nw") return { x: bounds.x, y: bounds.y };
  if (handle === "ne") return { x: bounds.x + bounds.width, y: bounds.y };
  if (handle === "se") return { x: bounds.x + bounds.width, y: bounds.y + bounds.height };
  return { x: bounds.x, y: bounds.y + bounds.height };
}

/**
 * Compute a new selection rectangle while dragging a corner handle.
 * The opposite corner stays fixed; the dragged corner follows `current`.
 */
export function resizeBoundsFromHandle(
  initial: EditorRect,
  handle: ResizeHandle,
  current: EditorPoint,
  minimumSize = 8,
): EditorRect {
  const min = Math.max(1, minimumSize);
  const anchor = resizeHandlePoint(initial, oppositeResizeHandle(handle));
  const width = Math.max(min, Math.abs(current.x - anchor.x));
  const height = Math.max(min, Math.abs(current.y - anchor.y));
  // Prefer the side the pointer is on when past the anchor so the box can flip.
  const flippedX = current.x < anchor.x;
  const flippedY = current.y < anchor.y;
  return {
    x: flippedX ? anchor.x - width : anchor.x,
    y: flippedY ? anchor.y - height : anchor.y,
    width,
    height,
  };
}

export function oppositeResizeHandle(handle: ResizeHandle): ResizeHandle {
  if (handle === "nw") return "se";
  if (handle === "ne") return "sw";
  if (handle === "se") return "nw";
  return "ne";
}

/**
 * Map an element so its content scales from `initialBounds` into `nextBounds`.
 * Used while dragging selection handles (text, shapes, paths, images).
 */
export function resizeElement(
  element: ScreenshotElement,
  initialBounds: EditorRect,
  nextBounds: EditorRect,
): ScreenshotElement {
  const scaleX = nextBounds.width / Math.max(1, initialBounds.width);
  const scaleY = nextBounds.height / Math.max(1, initialBounds.height);
  const mapPoint = (point: EditorPoint): EditorPoint => ({
    x: nextBounds.x + (point.x - initialBounds.x) * scaleX,
    y: nextBounds.y + (point.y - initialBounds.y) * scaleY,
  });

  if (element.kind === "image") {
    const topLeft = mapPoint({ x: element.x, y: element.y });
    return {
      ...element,
      x: topLeft.x,
      y: topLeft.y,
      width: Math.max(1, element.width * scaleX),
      height: Math.max(1, element.height * scaleY),
    };
  }

  if (element.kind === "text") {
    // Text boxes keep font size stable; resize only changes wrap width + origin.
    // Height of the selection follows wrapped content after the gesture ends.
    return {
      ...element,
      x: nextBounds.x,
      y: nextBounds.y,
      width: Math.max(minTextBoxWidth(element.fontSize), nextBounds.width),
    };
  }

  if (element.kind === "shape") {
    const start = mapPoint({ x: element.x, y: element.y });
    const end = mapPoint({ x: element.endX, y: element.endY });
    return {
      ...element,
      x: start.x,
      y: start.y,
      endX: end.x,
      endY: end.y,
      controls: element.controls.map(mapPoint),
    };
  }

  const origin = mapPoint({ x: element.x, y: element.y });
  return {
    ...element,
    x: origin.x,
    y: origin.y,
    points: element.points.map(mapPoint),
  };
}

export function resizeCursor(handle: ResizeHandle): string {
  if (handle === "nw" || handle === "se") return "nwse-resize";
  return "nesw-resize";
}

export function elementBounds(element: ScreenshotElement): EditorRect {
  if (element.kind === "image") {
    return {
      x: element.x,
      y: element.y,
      width: element.width,
      height: element.height,
    };
  }
  if (element.kind === "text") {
    const boxWidth = Math.max(minTextBoxWidth(element.fontSize), element.width);
    const lines = wrapTextLines(element.text, boxWidth, element.fontSize);
    return {
      x: element.x,
      y: element.y,
      width: boxWidth,
      height: Math.max(1, lines.length) * element.fontSize * TEXT_LINE_HEIGHT_RATIO,
    };
  }
  if (element.kind === "shape") {
    return shapeElementBounds(element);
  }
  if (element.points.length === 0) {
    return { x: element.x, y: element.y, width: 1, height: 1 };
  }
  const xs = element.points.map(({ x }) => x);
  const ys = element.points.map(({ y }) => y);
  const padding = Math.max(4, element.style.strokeWidth);
  const left = Math.min(...xs);
  const top = Math.min(...ys);
  return {
    x: left - padding,
    y: top - padding,
    width: Math.max(1, Math.max(...xs) - left) + padding * 2,
    height: Math.max(1, Math.max(...ys) - top) + padding * 2,
  };
}

export function hitTestElement(
  elements: ScreenshotElement[],
  point: EditorPoint,
  tolerance = 8,
): ScreenshotElement | null {
  for (let index = elements.length - 1; index >= 0; index -= 1) {
    const element = elements[index];
    if (!element.visible || element.locked) continue;
    const bounds = elementBounds(element);
    if (
      point.x >= bounds.x - tolerance
      && point.x <= bounds.x + bounds.width + tolerance
      && point.y >= bounds.y - tolerance
      && point.y <= bounds.y + bounds.height + tolerance
    ) {
      return element;
    }
  }
  return null;
}

export function outputDimensions(
  documentWidth: number,
  documentHeight: number,
  requestedWidth: number,
): { width: number; height: number } {
  const width = clamp(Math.round(requestedWidth), 1, 32_768);
  return {
    width,
    height: clamp(Math.round(width * documentHeight / Math.max(1, documentWidth)), 1, 32_768),
  };
}

export type ScreenshotExportFormat = "png" | "jpeg" | "webp";

/**
 * Load a dropped/picked image file into an HTMLImageElement.
 *
 * Uses a blob object URL first. If that fails (e.g. CSP without `blob:` in
 * img-src), falls back to a data URL so imports still work.
 *
 * When the browser `File` is empty or unreadable (common for same-app drops
 * from a native preview drag), `preparedBytes` can supply the PNG staged for
 * that drag so the editor still imports the image.
 */
export async function loadImageFile(
  file: File,
  options?: {
    preparedBytes?: () => Promise<Uint8Array | number[]>;
  },
): Promise<HTMLImageElement> {
  const loadFromBytes = async (bytes: Uint8Array | number[]): Promise<HTMLImageElement> => {
    const source = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    if (source.byteLength === 0) {
      throw new Error(`${file.name} could not be loaded.`);
    }
    // Copy into a fresh ArrayBuffer so File/BlobPart typing accepts the payload
    // across TypeScript DOM lib variants (SharedArrayBuffer vs ArrayBuffer).
    const payload = Uint8Array.from(source);
    const restored = new File([payload], file.name, {
      type: file.type || "image/png",
    });
    return decodeFileSources(restored);
  };

  if (file.size > 0) {
    try {
      return await decodeFileSources(file);
    } catch {
      // Fall through to prepared drag bytes when present.
    }
  }

  if (options?.preparedBytes) {
    try {
      return await loadFromBytes(await options.preparedBytes());
    } catch {
      // Prefer the original user-facing error below.
    }
  }

  throw new Error(`${file.name} could not be loaded.`);
}

async function decodeFileSources(file: File): Promise<HTMLImageElement> {
  const blobUrl = URL.createObjectURL(file);
  try {
    return await decodeImageSource(blobUrl);
  } catch {
    URL.revokeObjectURL(blobUrl);
  }

  const dataUrl = await readFileAsDataUrl(file);
  return decodeImageSource(dataUrl);
}

function decodeImageSource(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    // blob:/data: sources are same-origin; do not mark anonymous or decode fails.
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("image decode failed"));
    image.src = src;
  });
}

function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      if (typeof reader.result === "string") resolve(reader.result);
      else reject(new Error("Could not read the image file."));
    };
    reader.onerror = () => reject(new Error("Could not read the image file."));
    reader.readAsDataURL(file);
  });
}

/**
 * Browser-side encode used only for a live export size estimate. Final save
 * still goes through the Rust encoder, so this is approximate.
 */
export async function estimateCanvasExportBytes(
  canvas: HTMLCanvasElement,
  format: ScreenshotExportFormat,
  jpegQuality: number,
): Promise<number> {
  const mimeType = format === "jpeg"
    ? "image/jpeg"
    : format === "webp"
      ? "image/webp"
      : "image/png";
  const quality = format === "jpeg"
    ? Math.min(1, Math.max(0.4, jpegQuality / 100))
    : undefined;
  const blob = await new Promise<Blob>((resolve, reject) => {
    canvas.toBlob((result) => {
      if (result) resolve(result);
      else reject(new Error("The edited image could not be encoded for size estimation."));
    }, mimeType, quality);
  });
  return blob.size;
}

export function imageSizeAtWidth(
  element: EditorImageElement,
  width: number,
): { width: number; height: number } {
  const nextWidth = Math.max(1, Math.round(width));
  return {
    width: nextWidth,
    height: Math.max(1, Math.round(nextWidth * element.naturalHeight / element.naturalWidth)),
  };
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value));
}
