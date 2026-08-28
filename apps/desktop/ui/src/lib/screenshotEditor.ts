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
  | "pen"
  /** Magic wand + erase/restore brushes for punching alpha on image layers. */
  | "remove-bg";

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

/** The eight lossless right-angle orientations available to an image layer. */
export type ImageOrientation =
  | "normal"
  | "rotate-90"
  | "rotate-180"
  | "rotate-270"
  | "flip-horizontal"
  | "flip-vertical"
  | "transpose"
  | "transverse";

export type ImageTransformAction =
  | "rotate-clockwise"
  | "rotate-counterclockwise"
  | "flip-horizontal"
  | "flip-vertical";

export type ImageOrientationMatrix = Readonly<{
  a: number;
  b: number;
  c: number;
  d: number;
}>;

export type EditorImageElement = EditorElementBase & {
  kind: "image";
  source: "background" | "imported";
  src: string;
  /** Lossless source-pixel orientation; omitted documents are treated as normal. */
  orientation?: ImageOrientation;
  /**
   * Bitmap frozen at the first remove-background edit. The restore brush
   * paints from this source; omitted/`null` until the layer has been alpha-edited.
   */
  originalSrc?: string | null;
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
  /**
   * When true, typing grows the box with the longest line (a normal text field)
   * instead of wrapping inside the current width. New text starts this way;
   * older documents omit it and keep a fixed wrap width. Scaling an auto-width
   * label changes type size and keeps the plate hugging the glyphs.
   */
  autoWidth?: boolean;
  fontFamily: "sans" | "serif" | "mono" | "rounded";
  bold: boolean;
  italic: boolean;
  align: "left" | "center" | "right";
  color: string;
  background: string | null;
  /** Hollow text whose glyph edges use `color` instead of a solid fill. */
  outlined: boolean;
  /** Capsule-like background used by the Rounded Box preset. */
  roundedBackground: boolean;
};

export type TextStylePreset =
  | "standard"
  | "rounded"
  | "outlined"
  | "mono"
  | "box"
  | "mono-box"
  | "rounded-box";

export const DEFAULT_TEXT_BOX_BACKGROUND = "#111318";

/** Identify the closest named style represented by a text layer. */
export function textStylePreset(element: EditorTextElement): TextStylePreset {
  if (element.outlined && element.background === null) return "outlined";
  if (element.background !== null) {
    if (element.fontFamily === "rounded" && element.roundedBackground) {
      return "rounded-box";
    }
    if (element.fontFamily === "mono") return "mono-box";
    return "box";
  }
  if (element.fontFamily === "rounded") return "rounded";
  if (element.fontFamily === "mono") return "mono";
  return "standard";
}

/** Apply one of the named text treatments without changing content or colors. */
export function applyTextStylePreset(
  element: EditorTextElement,
  preset: TextStylePreset,
): EditorTextElement {
  const boxBackground = element.background ?? DEFAULT_TEXT_BOX_BACKGROUND;
  if (preset === "rounded") {
    return {
      ...element,
      fontFamily: "rounded",
      background: null,
      outlined: false,
      roundedBackground: false,
    };
  }
  if (preset === "outlined") {
    return {
      ...element,
      fontFamily: "sans",
      background: null,
      outlined: true,
      roundedBackground: false,
    };
  }
  if (preset === "mono") {
    return {
      ...element,
      fontFamily: "mono",
      background: null,
      outlined: false,
      roundedBackground: false,
    };
  }
  if (preset === "box") {
    return {
      ...element,
      fontFamily: "sans",
      background: boxBackground,
      outlined: false,
      roundedBackground: false,
    };
  }
  if (preset === "mono-box") {
    return {
      ...element,
      fontFamily: "mono",
      background: boxBackground,
      outlined: false,
      roundedBackground: false,
    };
  }
  if (preset === "rounded-box") {
    return {
      ...element,
      fontFamily: "rounded",
      background: boxBackground,
      outlined: false,
      roundedBackground: true,
    };
  }
  return {
    ...element,
    fontFamily: "sans",
    background: null,
    outlined: false,
    roundedBackground: false,
  };
}

/** Line box height as a multiple of fontSize (matches canvas + inline editor). */
export const TEXT_LINE_HEIGHT_RATIO = 1.25;

/**
 * Approximate average glyph advance relative to fontSize for sans-like faces.
 * Used when a Canvas measureText context is not available (hit tests, bounds).
 */
export const TEXT_CHAR_WIDTH_RATIO = 0.56;

/**
 * Horizontal padding of a solid/rounded text background beyond the layout box.
 * Kept in lockstep with canvas paint + inline editor chrome so trim/selection
 * include the full bubble, not just the glyph box.
 */
export const TEXT_BACKGROUND_PAD_X_RATIO = 0.22;

/**
 * Vertical padding of a solid/rounded text background beyond the layout box.
 * Symmetric so the label sits centered in the bubble.
 */
export const TEXT_BACKGROUND_PAD_Y_RATIO = 0.2;

/**
 * Extra downward shift when glyph bounding boxes are unavailable.
 * Latin caps sit high in the em square; this keeps labels optically centered
 * in rounded-box plates instead of hugging the top.
 */
export const TEXT_OPTICAL_CENTER_NUDGE_RATIO = 0.07;

/** Metrics used to sit glyphs in the vertical center of a line box. */
export type TextGlyphMetrics = {
  actualBoundingBoxAscent?: number;
  actualBoundingBoxDescent?: number;
};

/**
 * fillText Y and baseline so the painted letters sit in the middle of the
 * line box (and therefore the background plate), not at the em-square top.
 */
export function textGlyphDrawY(
  lineTop: number,
  fontSize: number,
  lineIndex: number,
  metrics?: TextGlyphMetrics | null,
): { y: number; baseline: "alphabetic" | "middle" } {
  const lineHeight = fontSize * TEXT_LINE_HEIGHT_RATIO;
  const mid = lineTop + lineIndex * lineHeight + lineHeight / 2;
  const ascent = metrics?.actualBoundingBoxAscent;
  const descent = metrics?.actualBoundingBoxDescent;
  if (
    typeof ascent === "number"
    && typeof descent === "number"
    && Number.isFinite(ascent)
    && Number.isFinite(descent)
    && ascent + descent > 1
  ) {
    return { y: mid + (ascent - descent) / 2, baseline: "alphabetic" };
  }
  return {
    y: mid + fontSize * TEXT_OPTICAL_CENTER_NUDGE_RATIO,
    baseline: "middle",
  };
}

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
    Math.round(fontSize * 2.2),
    Math.ceil(estimateTextWidth(sample, fontSize) + fontSize * 0.4),
  );
}

/** True when typing should grow the layout box instead of wrapping. */
export function isAutoWidthText(
  element: Pick<EditorTextElement, "autoWidth">,
): boolean {
  return element.autoWidth === true;
}

/**
 * Width of an auto-growing text box: the longest line, plus room for the caret.
 * Explicit newlines still create extra lines; words do not wrap to fit a column.
 */
export function fittedAutoWidthTextBox(
  text: string,
  fontSize: number,
  measure: (line: string) => number = (line) => estimateTextWidth(line, fontSize),
): number {
  const lines = text.split("\n");
  let widest = 0;
  for (const line of lines) {
    widest = Math.max(widest, measure(line.length > 0 ? line : " "));
  }
  if (widest <= 0) widest = measure(" ");
  return Math.max(
    minTextBoxWidth(fontSize),
    Math.ceil(widest + fontSize * 0.35),
  );
}

/**
 * Grow (or shrink) an auto-width text layer to fit its current content.
 * Left-aligned boxes grow to the right; center and right keep their anchor.
 */
export function fitAutoWidthTextElement(
  element: EditorTextElement,
  measure?: (line: string) => number,
): EditorTextElement {
  if (!isAutoWidthText(element)) return element;
  const nextWidth = fittedAutoWidthTextBox(element.text, element.fontSize, measure);
  if (Math.abs(nextWidth - element.width) < 0.5) {
    return nextWidth === element.width ? element : { ...element, width: nextWidth };
  }
  const delta = nextWidth - element.width;
  const x = element.align === "center"
    ? element.x - delta / 2
    : element.align === "right"
      ? element.x - delta
      : element.x;
  return { ...element, width: nextWidth, x };
}

/** Background inset around the text layout box (document pixels). */
export function textBackgroundPad(fontSize: number): { x: number; y: number } {
  return {
    x: fontSize * TEXT_BACKGROUND_PAD_X_RATIO,
    y: fontSize * TEXT_BACKGROUND_PAD_Y_RATIO,
  };
}

/** True when a text style paints a solid plate behind the glyphs. */
export function textHasBackgroundPlate(element: Pick<EditorTextElement, "background">): boolean {
  return element.background !== null && element.background !== "";
}

/**
 * Layout size of wrapped text (no background padding).
 * Origin stays at `element.x` / `element.y`.
 */
export function textContentSize(
  element: Pick<EditorTextElement, "text" | "width" | "fontSize">,
): { width: number; height: number } {
  const boxWidth = Math.max(minTextBoxWidth(element.fontSize), element.width);
  const lines = wrapTextLines(element.text, boxWidth, element.fontSize);
  return {
    width: boxWidth,
    height: Math.max(1, lines.length) * element.fontSize * TEXT_LINE_HEIGHT_RATIO,
  };
}

/** Whether new text of this preset should default to centered alignment. */
export function textPresetPrefersCenter(preset: TextStylePreset): boolean {
  return preset === "rounded-box" || preset === "box" || preset === "mono-box";
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
   * Used by open stroke shapes (line and arrow); closed shapes keep this empty.
   * Empty = straight shaft; one point = quadratic curve; more = smooth multi-segment path.
   */
  controls: EditorPoint[];
  style: ElementStyle;
};

/**
 * Open stroke shapes that support multi-point Bezier curve controls
 * (start → free controls → end), same path model for line and arrow.
 */
export type CurveableStrokeShape = Extract<ShapeKind, "line" | "arrow">;

/** True for line and arrow shapes that accept free curve control points. */
export function isCurveableStrokeShape(
  element: Pick<EditorShapeElement, "shape"> | ShapeKind,
): boolean {
  const shape = typeof element === "string" ? element : element.shape;
  return shape === "line" || shape === "arrow";
}

/** Editable handle on a selected line/arrow (endpoints, controls, or starter dots). */
export type ArrowHandle =
  | { kind: "start" }
  | { kind: "end" }
  | { kind: "control"; index: number }
  /** One of the three on-stroke starter dots shown before a stroke has controls. */
  | { kind: "starter-control"; index: number };

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

const IMAGE_ORIENTATION_MATRICES: Record<ImageOrientation, ImageOrientationMatrix> = {
  normal: { a: 1, b: 0, c: 0, d: 1 },
  "rotate-90": { a: 0, b: 1, c: -1, d: 0 },
  "rotate-180": { a: -1, b: 0, c: 0, d: -1 },
  "rotate-270": { a: 0, b: -1, c: 1, d: 0 },
  "flip-horizontal": { a: -1, b: 0, c: 0, d: 1 },
  "flip-vertical": { a: 1, b: 0, c: 0, d: -1 },
  transpose: { a: 0, b: 1, c: 1, d: 0 },
  transverse: { a: 0, b: -1, c: -1, d: 0 },
};

const IMAGE_TRANSFORM_MATRICES: Record<ImageTransformAction, ImageOrientationMatrix> = {
  "rotate-clockwise": IMAGE_ORIENTATION_MATRICES["rotate-90"],
  "rotate-counterclockwise": IMAGE_ORIENTATION_MATRICES["rotate-270"],
  "flip-horizontal": IMAGE_ORIENTATION_MATRICES["flip-horizontal"],
  "flip-vertical": IMAGE_ORIENTATION_MATRICES["flip-vertical"],
};

const IMAGE_ORIENTATION_BY_MATRIX = new Map(
  Object.entries(IMAGE_ORIENTATION_MATRICES).map(([orientation, matrix]) => [
    `${matrix.a},${matrix.b},${matrix.c},${matrix.d}`,
    orientation as ImageOrientation,
  ]),
);

export function imageOrientationMatrix(
  orientation: ImageOrientation | null | undefined,
): ImageOrientationMatrix {
  return IMAGE_ORIENTATION_MATRICES[orientation ?? "normal"];
}

/** True when the oriented bitmap's displayed width/height are source height/width. */
export function imageOrientationSwapsAxes(
  orientation: ImageOrientation | null | undefined,
): boolean {
  const matrix = imageOrientationMatrix(orientation);
  return matrix.a === 0 && matrix.d === 0;
}

/** Source-oriented dimensions inside the image layer's current display bounds. */
export function imageSourceDisplaySize(
  element: Pick<EditorImageElement, "width" | "height" | "orientation">,
): { width: number; height: number } {
  return imageOrientationSwapsAxes(element.orientation)
    ? { width: element.height, height: element.width }
    : { width: element.width, height: element.height };
}

function composeImageOrientation(
  operation: ImageOrientationMatrix,
  current: ImageOrientationMatrix,
): ImageOrientation {
  // Canvas matrices represent [[a,c],[b,d]]. New user actions operate in the
  // currently displayed axes, so compose them on the left: operation × current.
  const matrix: ImageOrientationMatrix = {
    a: operation.a * current.a + operation.c * current.b,
    b: operation.b * current.a + operation.d * current.b,
    c: operation.a * current.c + operation.c * current.d,
    d: operation.b * current.c + operation.d * current.d,
  };
  const orientation = IMAGE_ORIENTATION_BY_MATRIX.get(
    `${matrix.a},${matrix.b},${matrix.c},${matrix.d}`,
  );
  if (!orientation) throw new Error("Unsupported image orientation.");
  return orientation;
}

/** Rotate or mirror an image around its displayed center without resampling pixels. */
export function transformImageElement(
  element: EditorImageElement,
  action: ImageTransformAction,
): EditorImageElement {
  const orientation = composeImageOrientation(
    IMAGE_TRANSFORM_MATRICES[action],
    imageOrientationMatrix(element.orientation),
  );
  const rotates = action === "rotate-clockwise" || action === "rotate-counterclockwise";
  const width = rotates ? element.height : element.width;
  const height = rotates ? element.width : element.height;
  const centerX = element.x + element.width / 2;
  const centerY = element.y + element.height / 2;
  return {
    ...element,
    // Keep identity transforms JSON-equivalent to documents created before
    // image orientation existed, so paired flips/four rotations are true no-ops.
    orientation: orientation === "normal" ? undefined : orientation,
    x: centerX - width / 2,
    y: centerY - height / 2,
    width,
    height,
  };
}

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
      originalSrc: null,
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

/** Below this size, Shift-lock treats the crop as unsized and snaps to 1:1. */
const CROP_SHIFT_LOCK_MIN_SIZE = 8;

/**
 * Aspect used while dragging a crop.
 *
 * A sidebar preset (`16:9`, `1:1`, …) always wins. When the preset is free,
 * holding Shift locks the live crop's current ratio (1:1 if the box is still
 * tiny) and keeps that snapshot until Shift is released.
 *
 * Pass `liveRect` (the last crop box) so Shift-down freezes the ratio you
 * already drew, not the unconstrained pointer on this frame.
 *
 * Command/Ctrl is not used here — the editor already pans with those keys.
 */
export function cropDragAspectRatio(options: {
  preset: string;
  shiftKey: boolean;
  origin: EditorPoint;
  current: EditorPoint;
  bounds: Pick<ScreenshotDocument, "width" | "height">;
  shiftAspect: number | null;
  liveRect?: EditorRect | null;
}): { aspectRatio: number | null; shiftAspect: number | null } {
  const preset = parseCropAspectPreset(options.preset);
  if (preset !== null) {
    return { aspectRatio: preset, shiftAspect: null };
  }
  if (!options.shiftKey) {
    return { aspectRatio: null, shiftAspect: null };
  }
  if (options.shiftAspect && options.shiftAspect > 0) {
    return { aspectRatio: options.shiftAspect, shiftAspect: options.shiftAspect };
  }
  const shiftAspect = cropAspectFromLiveRect(options.liveRect)
    ?? shiftLockedCropAspect(options.origin, options.current, options.bounds);
  return { aspectRatio: shiftAspect, shiftAspect };
}

function cropAspectFromLiveRect(rect: EditorRect | null | undefined): number | null {
  if (
    !rect
    || rect.width < CROP_SHIFT_LOCK_MIN_SIZE
    || rect.height < CROP_SHIFT_LOCK_MIN_SIZE
  ) {
    return null;
  }
  return rect.width / rect.height;
}

function parseCropAspectPreset(value: string): number | null {
  if (!value || value === "free") return null;
  const parts = value.split(":").map(Number);
  if (parts.length !== 2) return null;
  const [width, height] = parts;
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) {
    return null;
  }
  return width / height;
}

/**
 * Ratio of the current (canvas-clamped) crop box. Tiny boxes lock to a square
 * so holding Shift from the first pixel behaves like a 1:1 constraint.
 */
export function shiftLockedCropAspect(
  origin: EditorPoint,
  current: EditorPoint,
  bounds: Pick<ScreenshotDocument, "width" | "height">,
): number {
  const live = boundedCropRect(origin, current, bounds, null);
  if (live.width >= CROP_SHIFT_LOCK_MIN_SIZE && live.height >= CROP_SHIFT_LOCK_MIN_SIZE) {
    return live.width / live.height;
  }
  return 1;
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

/**
 * Live hover preview for **Trim edges**: which current-canvas margins would be
 * discarded. Content that already overhangs is not “removed” from the frame
 * (the canvas grows instead), so only positive interior margins are returned.
 * `null` when trim is a no-op or there is nothing to cut away on-canvas.
 */
export type CanvasTrimMarginPreview = {
  /** Portion of the current canvas that remains after trim (document coords). */
  keepRect: EditorRect;
  /** Pixel strip sizes that will be removed from each edge. */
  margins: { top: number; right: number; bottom: number; left: number };
  /** Edges with a positive margin (for edge glow / particles). */
  edges: ImageSnapEdge[];
};

export function canvasTrimMarginPreview(
  document: ScreenshotDocument,
  padding = 0,
): CanvasTrimMarginPreview | null {
  if (trimDocumentToContent(document, padding) === document) return null;

  const content = visibleContentBounds(document);
  if (!content) return null;

  const safePadding = Math.max(0, Math.round(padding));
  // Keep rect is the content bounds (plus padding) clamped to the current canvas.
  // Margins outside that rect on the *current* canvas are what get discarded.
  const keepLeft = Math.max(0, Math.floor(content.x) - safePadding);
  const keepTop = Math.max(0, Math.floor(content.y) - safePadding);
  const keepRight = Math.min(
    document.width,
    Math.ceil(content.x + content.width) + safePadding,
  );
  const keepBottom = Math.min(
    document.height,
    Math.ceil(content.y + content.height) + safePadding,
  );

  if (keepRight <= keepLeft || keepBottom <= keepTop) return null;

  const margins = {
    left: keepLeft,
    top: keepTop,
    right: Math.max(0, document.width - keepRight),
    bottom: Math.max(0, document.height - keepBottom),
  };

  const edges: ImageSnapEdge[] = [];
  if (margins.top > 0) edges.push("top");
  if (margins.right > 0) edges.push("right");
  if (margins.bottom > 0) edges.push("bottom");
  if (margins.left > 0) edges.push("left");
  if (edges.length === 0) return null;

  return {
    keepRect: {
      x: keepLeft,
      y: keepTop,
      width: keepRight - keepLeft,
      height: keepBottom - keepTop,
    },
    margins,
    edges,
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

/**
 * How an imported image is scaled into the document.
 * - `overlay`: cap at ~65% of the canvas so stacked drops stay manageable.
 * - `natural`: keep source pixels 1:1 so edge-snapped screenshots match the
 *   backdrop when they share the same capture scale (no surprise downscale).
 */
export type ImportedImageFit = "overlay" | "natural";

export function positionImportedImage(
  naturalWidth: number,
  naturalHeight: number,
  document: Pick<ScreenshotDocument, "width" | "height">,
  dropPoint?: EditorPoint,
  fit: ImportedImageFit = "overlay",
): EditorRect {
  const safeWidth = Math.max(1, naturalWidth);
  const safeHeight = Math.max(1, naturalHeight);
  let scale = 1;
  if (fit === "overlay") {
    const maximumWidth = Math.max(160, document.width * 0.65);
    const maximumHeight = Math.max(120, document.height * 0.65);
    scale = Math.min(1, maximumWidth / safeWidth, maximumHeight / safeHeight);
  }
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

/** Distance from a point to the nearest edge of a rect (0 when inside). */
function distanceToRect(point: EditorPoint, rect: EditorRect): number {
  const dx = point.x < rect.x
    ? rect.x - point.x
    : point.x > rect.x + rect.width
      ? point.x - (rect.x + rect.width)
      : 0;
  const dy = point.y < rect.y
    ? rect.y - point.y
    : point.y > rect.y + rect.height
      ? point.y - (rect.y + rect.height)
      : 0;
  return Math.hypot(dx, dy);
}

/**
 * Pick the layer bounds that an imported image should snap against.
 *
 * When a pointer sample is available (live drag), use the top-most *visible*
 * image under that point so only one layer highlights at a time — never a
 * buried layer under an overlapping one, and never a layer the cursor is not
 * over. Outside every image, snap to the closest visible image (so edge-band
 * placement still works just outside a layer).
 *
 * Without a pointer sample (drop before any dragover), fall back to the
 * selected visible image, then the front-most visible image, then the canvas.
 */
export function resolveImageDropTarget(
  document: Pick<ScreenshotDocument, "width" | "height" | "elements">,
  selectedId: string | null,
  point?: EditorPoint,
): EditorRect {
  const canvas: EditorRect = {
    x: 0,
    y: 0,
    width: document.width,
    height: document.height,
  };

  if (point) {
    // Top-most image whose bounds contain the pointer (z-order = array order).
    for (let index = document.elements.length - 1; index >= 0; index -= 1) {
      const element = document.elements[index];
      if (element.kind !== "image" || !element.visible) continue;
      const bounds = elementBounds(element);
      if (
        point.x >= bounds.x
        && point.x <= bounds.x + bounds.width
        && point.y >= bounds.y
        && point.y <= bounds.y + bounds.height
      ) {
        return bounds;
      }
    }

    // Outside every image: closest layer (front-most wins ties) so edge snaps
    // just outside a partially-exposed layer still target that layer.
    let best: EditorRect | null = null;
    let bestDistance = Number.POSITIVE_INFINITY;
    for (let index = document.elements.length - 1; index >= 0; index -= 1) {
      const element = document.elements[index];
      if (element.kind !== "image" || !element.visible) continue;
      const bounds = elementBounds(element);
      const distance = distanceToRect(point, bounds);
      if (distance < bestDistance) {
        bestDistance = distance;
        best = bounds;
      }
    }
    if (best) return best;
    return canvas;
  }

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

  return canvas;
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
  const target = resolveImageDropTarget(document, selectedId, point);
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
 *
 * Edge snaps use natural 1:1 pixels so composing similar screenshots keeps them
 * the same size; stack-on-top still uses the overlay cap so large imports do
 * not fully cover the canvas.
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
  const fit: ImportedImageFit = edge === "stack" ? "overlay" : "natural";
  const centered = positionImportedImage(
    naturalWidth,
    naturalHeight,
    document,
    edge === "stack"
      ? stackCenter
      : { x: target.x + target.width / 2, y: target.y + target.height / 2 },
    fit,
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

/** Canvas growth padding after an image drop. Edge collages sit flush; stack keeps a small margin. */
export function imageDropExpandPadding(edge: ImageDropPlacement): number {
  return edge === "stack" ? 24 : 0;
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

/** Whether the selected layer can merge into the layer immediately below it. */
export function canMergeLayerDown(
  elements: readonly ScreenshotElement[],
  selectedId: string | null | undefined,
): boolean {
  if (!selectedId) return false;
  const index = elements.findIndex((element) => element.id === selectedId);
  if (index <= 0) return false;
  const selected = elements[index];
  const below = elements[index - 1];
  return !selected.locked && !below.locked;
}

/** Merge Visible needs at least two currently visible layers. */
export function canMergeVisibleLayers(elements: readonly ScreenshotElement[]): boolean {
  return elements.filter((element) => element.visible).length >= 2;
}

/**
 * Flatten needs more than one layer, or a single unlocked layer with a solid
 * canvas background still worth baking in.
 */
export function canFlattenLayers(
  elements: readonly ScreenshotElement[],
  background: string | null = null,
): boolean {
  if (elements.length >= 2) return true;
  return elements.length === 1 && background != null;
}

/**
 * Replace the selected layer and the layer below it with a pre-rasterized image.
 * Caller paints selected + below in stack order before invoking this.
 */
export function applyMergeLayerDown(
  document: ScreenshotDocument,
  selectedId: string,
  merged: EditorImageElement,
): ScreenshotDocument {
  const index = document.elements.findIndex((element) => element.id === selectedId);
  if (index <= 0) return document;
  const elements = [...document.elements];
  elements.splice(index - 1, 2, merged);
  return { ...document, elements };
}

/**
 * Collapse every visible layer into `merged`, keeping hidden layers in place.
 * The merged image sits where the bottom-most visible layer was.
 */
export function applyMergeVisibleLayers(
  document: ScreenshotDocument,
  merged: EditorImageElement,
): ScreenshotDocument {
  const elements: ScreenshotElement[] = [];
  let inserted = false;
  for (const element of document.elements) {
    if (!element.visible) {
      elements.push(element);
      continue;
    }
    if (!inserted) {
      elements.push(merged);
      inserted = true;
    }
  }
  if (!inserted) return document;
  return { ...document, elements };
}

/**
 * Bake the canvas background and all visible layers into a single locked
 * background image. Hidden layers are discarded (Photoshop-style flatten).
 */
export function applyFlattenLayers(
  document: ScreenshotDocument,
  merged: EditorImageElement,
): ScreenshotDocument {
  return {
    ...document,
    // Background is already painted into `merged` when provided by the caller.
    background: null,
    elements: [{ ...merged, locked: true, source: "background" }],
  };
}

/** Default name for a rasterized merge of the given layers. */
export function mergedLayerName(layers: readonly ScreenshotElement[]): string {
  const image = layers.find((layer): layer is EditorImageElement => layer.kind === "image");
  if (image) return image.name;
  return "Merged";
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

/** Ordered vertices for a line/arrow stroke: start → free controls → end. */
export function arrowVertices(element: EditorShapeElement): EditorPoint[] {
  return [
    { x: element.x, y: element.y },
    ...element.controls,
    { x: element.endX, y: element.endY },
  ];
}

/** Midpoint of the start→end chord (default mid handle when the stroke is straight). */
export function arrowDefaultMidHandle(element: EditorShapeElement): EditorPoint {
  return {
    x: (element.x + element.endX) / 2,
    y: (element.y + element.endY) / 2,
  };
}

/**
 * Three evenly-spaced starter controls for a straight line/arrow. They remain
 * virtual until one is dragged, keeping the stored stroke straight while making
 * multi-point shaping immediately discoverable.
 */
export function arrowStarterControls(element: EditorShapeElement): EditorPoint[] {
  return [0.25, 0.5, 0.75].map((progress) => ({
    x: element.x + (element.endX - element.x) * progress,
    y: element.y + (element.endY - element.y) * progress,
  }));
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
 * Sample points along a line/arrow shaft for hit-testing and closest-point queries.
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
 * Closest sampled point on a line/arrow path and the control-array index where a
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

/** Straight-line length from start to tip (used to keep the head in proportion). */
export function arrowChordLength(
  element: Pick<EditorShapeElement, "x" | "y" | "endX" | "endY">,
): number {
  return Math.hypot(element.endX - element.x, element.endY - element.y);
}

/**
 * Drawn arrow-head wing length (must match the renderer in ScreenshotEditor).
 * Used for content bounds so trim/expand account for the head, not just the shaft.
 *
 * Head size follows stroke width. When `shaftLength` is given, the tip also
 * cannot exceed a fraction of the shaft — otherwise shrinking an arrow leaves a
 * giant point on a tiny body.
 */
export function arrowHeadLength(strokeWidth: number, shaftLength?: number): number {
  const fromStroke = Math.max(1, strokeWidth * 4.2);
  if (shaftLength == null || !(shaftLength > 0)) return fromStroke;
  // Keep the tip shorter than the remaining shaft so endpoint-shrink cannot
  // leave a creation-sized head on a stub.
  return Math.max(1, Math.min(fromStroke, shaftLength * 0.22));
}

/**
 * Arrowhead wing tip positions (matches the canvas renderer).
 * Used so content bounds expand only near the tip, not isotropically around the
 * whole shaft (which left large empty margins for trim / selection boxes).
 */
export function arrowHeadWingTips(
  end: EditorPoint,
  tangent: EditorPoint,
  strokeWidth: number,
  shaftLength?: number,
): [EditorPoint, EditorPoint] {
  const angle = Math.atan2(end.y - tangent.y, end.x - tangent.x);
  const length = arrowHeadLength(strokeWidth, shaftLength);
  return [
    {
      x: end.x - length * Math.cos(angle - Math.PI / 6),
      y: end.y - length * Math.sin(angle - Math.PI / 6),
    },
    {
      x: end.x - length * Math.cos(angle + Math.PI / 6),
      y: end.y - length * Math.sin(angle + Math.PI / 6),
    },
  ];
}

/**
 * Thin an arrow's stroke when its shaft is shortened (endpoint or corner scale)
 * so the head, which is derived from stroke width, shrinks with the body.
 * Lengthening keeps the original pen size.
 */
export function scaleArrowStrokeForLength(
  initial: EditorShapeElement,
  next: EditorShapeElement,
): EditorShapeElement {
  if (initial.shape !== "arrow" || next.shape !== "arrow") return next;
  const initialLength = Math.max(1, arrowChordLength(initial));
  const nextLength = arrowChordLength(next);
  if (nextLength >= initialLength - 0.5) return next;
  return {
    ...next,
    style: {
      ...next.style,
      strokeWidth: clamp(
        initial.style.strokeWidth * (nextLength / initialLength),
        1,
        80,
      ),
    },
  };
}

/**
 * Outward extent of a stroked path (half width + small AA slop).
 * Kept tight so selection boxes and Trim edges sit near the paint, not in a
 * large empty margin around thin strokes.
 */
function strokeExtent(strokeWidth: number): number {
  return Math.max(1, Math.ceil(strokeWidth / 2) + 1);
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
 * shaft stayed inside. Bounds therefore follow path samples for lines/arrows,
 * not the control handles.
 *
 * Arrowhead wings are measured at the tip only — never as an isotropic pad on
 * every side of the shaft (that left large empty selection / trim margins).
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

  if (isCurveableStrokeShape(element)) {
    // Dense samples so tight multi-point loops don't miss extrema.
    const samples = sampleArrowPath(element, 48);
    const points: EditorPoint[] = samples.length > 0
      ? [...samples]
      : [
        { x: element.x, y: element.y },
        { x: element.endX, y: element.endY },
      ];
    if (element.shape === "arrow") {
      const tip = { x: element.endX, y: element.endY };
      const wings = arrowHeadWingTips(
        tip,
        arrowHeadTangentPoint(element),
        element.style.strokeWidth,
        arrowChordLength(element),
      );
      points.push(tip, wings[0], wings[1]);
    }
    return boundsFromPoints(points, strokePad);
  }

  // Fallback for any future open stroke without the curve model.
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
  if (!isCurveableStrokeShape(element) || element.controls.length !== 1) return 0;
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
  if (!isCurveableStrokeShape(element)) return 0;
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
 * control; near-zero bend clears controls back to a straight stroke.
 */
export function arrowWithBend(
  element: EditorShapeElement,
  bend: number,
): EditorShapeElement {
  if (!isCurveableStrokeShape(element)) return element;
  if (Math.abs(bend) < 0.005) {
    return { ...element, controls: [] };
  }
  return {
    ...element,
    controls: [arrowControlFromBend(element, bend)],
  };
}

/** Insert a free control at `point`. */
export function insertArrowControl(
  element: EditorShapeElement,
  point: EditorPoint,
): EditorShapeElement | null {
  if (!isCurveableStrokeShape(element)) return null;
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
  if (!isCurveableStrokeShape(element)) return element;
  if (index < 0 || index >= element.controls.length) return element;
  return {
    ...element,
    controls: element.controls.filter((_, i) => i !== index),
  };
}

/**
 * Hit-test line/arrow edit handles. Order: free controls → endpoints → starter dots.
 * Returns null when the pointer is not on a handle.
 */
export function hitTestArrowHandle(
  element: EditorShapeElement,
  point: EditorPoint,
  handleRadius: number,
): ArrowHandle | null {
  if (!isCurveableStrokeShape(element)) return null;
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
    const starters = arrowStarterControls(element);
    for (let index = 0; index < starters.length; index += 1) {
      const starter = starters[index];
      // Slightly larger hit targets keep the on-path dots easy to grab.
      if (Math.hypot(point.x - starter.x, point.y - starter.y) <= radius * 1.15) {
        return { kind: "starter-control", index };
      }
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
  return handle?.kind === "control" || handle?.kind === "starter-control";
}

/**
 * Legacy single control-point position for one free control, or the default mid
 * when the stroke is straight. Prefer `arrowVertices` / free controls for new UI.
 */
export function arrowControlPoint(element: EditorShapeElement): EditorPoint | null {
  if (!isCurveableStrokeShape(element)) return null;
  if (element.controls.length === 1) return element.controls[0];
  if (element.controls.length === 0) return arrowDefaultMidHandle(element);
  return null;
}

/**
 * Hover/help copy for curve editing on a selected line or arrow.
 * Returns null when the pointer is not near a handle or the stroke path.
 */
export function curveStrokeHoverHint(
  element: EditorShapeElement,
  point: EditorPoint,
  handleRadius: number,
): string | null {
  if (!isCurveableStrokeShape(element) || element.locked) return null;
  const handle = hitTestArrowHandle(element, point, handleRadius);
  if (handle?.kind === "control") {
    return "Double-click to remove curve point";
  }
  if (handle?.kind === "starter-control") {
    return "Drag a dot to curve · Double-click the path to add points";
  }
  if (handle?.kind === "start" || handle?.kind === "end") {
    return "Drag to move endpoint";
  }

  const closest = closestPointOnArrow(element, point);
  const pathHitRadius = Math.max(
    handleRadius,
    element.style.strokeWidth * 2 + handleRadius * 0.6,
  );
  if (closest.distance > pathHitRadius) return null;
  return "Double-click to add a curve point";
}

/**
 * Selection resize grips: four corners plus mid-edge handles so dragging the
 * dashed selection border resizes, not only the corner squares.
 */
export type ResizeHandle =
  | "nw"
  | "n"
  | "ne"
  | "e"
  | "se"
  | "s"
  | "sw"
  | "w";

const RESIZE_HANDLES: ResizeHandle[] = [
  "nw",
  "n",
  "ne",
  "e",
  "se",
  "s",
  "sw",
  "w",
];

const RESIZE_CORNER_HANDLES: ResizeHandle[] = ["nw", "ne", "se", "sw"];

/**
 * Hit-test corner grips, mid-edge grips, and the dashed selection border itself.
 * `handleRadius` is in document pixels (scale for display zoom before calling).
 *
 * `grips: "corners"` is for labels that must scale as a unit: mid-edge squares
 * are ignored, and dragging the dashed border maps onto the nearest corner.
 */
export function hitTestResizeHandle(
  bounds: EditorRect,
  point: EditorPoint,
  handleRadius: number,
  grips: "all" | "corners" = "all",
): ResizeHandle | null {
  const radius = Math.max(4, handleRadius);
  const cornersOnly = grips === "corners";
  // Corners first so diagonal grips win over edge strips near the same pixel.
  for (const handle of RESIZE_CORNER_HANDLES) {
    const corner = resizeHandlePoint(bounds, handle);
    if (
      Math.abs(point.x - corner.x) <= radius
      && Math.abs(point.y - corner.y) <= radius
    ) {
      return handle;
    }
  }
  if (!cornersOnly) {
    for (const handle of RESIZE_HANDLES) {
      if (RESIZE_CORNER_HANDLES.includes(handle)) continue;
      const mid = resizeHandlePoint(bounds, handle);
      if (
        Math.abs(point.x - mid.x) <= radius
        && Math.abs(point.y - mid.y) <= radius
      ) {
        return handle;
      }
    }
  }

  // Dragging the dashed outline (not just the grip squares) also resizes.
  const left = bounds.x;
  const top = bounds.y;
  const right = bounds.x + bounds.width;
  const bottom = bounds.y + bounds.height;
  const inX = point.x >= left - radius && point.x <= right + radius;
  const inY = point.y >= top - radius && point.y <= bottom + radius;
  if (!inX || !inY) return null;

  const nearLeft = Math.abs(point.x - left) <= radius;
  const nearRight = Math.abs(point.x - right) <= radius;
  const nearTop = Math.abs(point.y - top) <= radius;
  const nearBottom = Math.abs(point.y - bottom) <= radius;
  const midX = left + bounds.width / 2;
  const midY = top + bounds.height / 2;

  if (cornersOnly) {
    if (nearTop && nearLeft) return "nw";
    if (nearTop && nearRight) return "ne";
    if (nearBottom && nearRight) return "se";
    if (nearBottom && nearLeft) return "sw";
    if (nearTop) return point.x >= midX ? "ne" : "nw";
    if (nearBottom) return point.x >= midX ? "se" : "sw";
    if (nearLeft) return point.y >= midY ? "sw" : "nw";
    if (nearRight) return point.y >= midY ? "se" : "ne";
    return null;
  }

  if (nearTop && !nearLeft && !nearRight) return "n";
  if (nearBottom && !nearLeft && !nearRight) return "s";
  if (nearLeft && !nearTop && !nearBottom) return "w";
  if (nearRight && !nearTop && !nearBottom) return "e";
  return null;
}

export function resizeHandlePoint(bounds: EditorRect, handle: ResizeHandle): EditorPoint {
  const midX = bounds.x + bounds.width / 2;
  const midY = bounds.y + bounds.height / 2;
  if (handle === "nw") return { x: bounds.x, y: bounds.y };
  if (handle === "n") return { x: midX, y: bounds.y };
  if (handle === "ne") return { x: bounds.x + bounds.width, y: bounds.y };
  if (handle === "e") return { x: bounds.x + bounds.width, y: midY };
  if (handle === "se") return { x: bounds.x + bounds.width, y: bounds.y + bounds.height };
  if (handle === "s") return { x: midX, y: bounds.y + bounds.height };
  if (handle === "sw") return { x: bounds.x, y: bounds.y + bounds.height };
  return { x: bounds.x, y: midY };
}

/**
 * True for the four corner grips. Mid-edge handles stay freeform even when
 * Shift is held — proportional lock only applies to corners.
 */
export function isResizeCornerHandle(handle: ResizeHandle): boolean {
  return RESIZE_CORNER_HANDLES.includes(handle);
}

/**
 * Compute a new selection rectangle while dragging a corner or edge handle.
 * The opposite edge/corner stays fixed; the dragged side follows `current`.
 *
 * When `lockAspectRatio` is true and `handle` is a corner, width and height
 * keep the initial aspect ratio (Shift-drag proportional scale). Edge handles
 * ignore the lock so mid-side drags remain single-axis.
 */
export function resizeBoundsFromHandle(
  initial: EditorRect,
  handle: ResizeHandle,
  current: EditorPoint,
  minimumSize = 8,
  lockAspectRatio = false,
): EditorRect {
  const min = Math.max(1, minimumSize);
  if (
    lockAspectRatio
    && isResizeCornerHandle(handle)
    && initial.width >= 1
    && initial.height >= 1
  ) {
    return resizeBoundsProportional(initial, handle, current, min);
  }

  const fixedLeft = initial.x;
  const fixedTop = initial.y;
  const fixedRight = initial.x + initial.width;
  const fixedBottom = initial.y + initial.height;

  const moveLeft = handle === "w" || handle === "nw" || handle === "sw";
  const moveRight = handle === "e" || handle === "ne" || handle === "se";
  const moveTop = handle === "n" || handle === "nw" || handle === "ne";
  const moveBottom = handle === "s" || handle === "sw" || handle === "se";

  let left = moveLeft ? current.x : fixedLeft;
  let right = moveRight ? current.x : fixedRight;
  let top = moveTop ? current.y : fixedTop;
  let bottom = moveBottom ? current.y : fixedBottom;

  if (left > right) {
    const swap = left;
    left = right;
    right = swap;
  }
  if (top > bottom) {
    const swap = top;
    top = bottom;
    bottom = swap;
  }

  let width = right - left;
  let height = bottom - top;
  if (width < min) {
    if (moveLeft && !moveRight) left = right - min;
    else if (moveRight && !moveLeft) right = left + min;
    else {
      const center = (left + right) / 2;
      left = center - min / 2;
      right = center + min / 2;
    }
    width = min;
  }
  if (height < min) {
    if (moveTop && !moveBottom) top = bottom - min;
    else if (moveBottom && !moveTop) bottom = top + min;
    else {
      const center = (top + bottom) / 2;
      top = center - min / 2;
      bottom = center + min / 2;
    }
    height = min;
  }

  return { x: left, y: top, width, height };
}

/**
 * Corner resize with a fixed opposite corner and locked aspect ratio.
 * The free corner tracks the pointer while width/height stay proportional.
 */
function resizeBoundsProportional(
  initial: EditorRect,
  handle: ResizeHandle,
  current: EditorPoint,
  min: number,
): EditorRect {
  const aspect = initial.width / initial.height;
  const anchor = resizeHandlePoint(initial, oppositeResizeHandle(handle));

  // Proposed free size from the fixed corner to the pointer (allow flip).
  let width = Math.max(Math.abs(current.x - anchor.x), 1e-6);
  let height = Math.max(Math.abs(current.y - anchor.y), 1e-6);

  if (width / height > aspect) {
    height = width / aspect;
  } else {
    width = height * aspect;
  }

  // Keep both axes at least `min` without breaking the aspect ratio.
  if (width < min || height < min) {
    if (aspect >= 1) {
      width = Math.max(min, width);
      height = width / aspect;
      if (height < min) {
        height = min;
        width = height * aspect;
      }
    } else {
      height = Math.max(min, height);
      width = height * aspect;
      if (width < min) {
        width = min;
        height = width / aspect;
      }
    }
  }

  const signX = current.x >= anchor.x ? 1 : -1;
  const signY = current.y >= anchor.y ? 1 : -1;
  return {
    x: signX > 0 ? anchor.x : anchor.x - width,
    y: signY > 0 ? anchor.y : anchor.y - height,
    width,
    height,
  };
}

export function oppositeResizeHandle(handle: ResizeHandle): ResizeHandle {
  if (handle === "nw") return "se";
  if (handle === "n") return "s";
  if (handle === "ne") return "sw";
  if (handle === "e") return "w";
  if (handle === "se") return "nw";
  if (handle === "s") return "n";
  if (handle === "sw") return "ne";
  return "e";
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
    // Side drags on a fixed wrap box change column width (reflow, same type size).
    // Auto-width labels and any height/corner drag scale type size instead of
    // stretching an empty plate around unchanged glyphs.
    const widthOnly = Math.abs(scaleY - 1) < 0.001 && Math.abs(scaleX - 1) >= 0.001;
    const heightOnly = Math.abs(scaleX - 1) < 0.001 && Math.abs(scaleY - 1) >= 0.001;
    const autoWidth = isAutoWidthText(element);
    if (widthOnly && !autoWidth) {
      const pad = textHasBackgroundPlate(element)
        ? textBackgroundPad(element.fontSize)
        : { x: 0, y: 0 };
      return {
        ...element,
        x: nextBounds.x + pad.x,
        y: nextBounds.y + pad.y,
        width: Math.max(
          minTextBoxWidth(element.fontSize),
          nextBounds.width - pad.x * 2,
        ),
        autoWidth: false,
      };
    }
    const fontScale = widthOnly
      ? scaleX
      : heightOnly
        ? scaleY
        : Math.min(Math.abs(scaleX), Math.abs(scaleY));
    const nextFontSize = clamp(
      Math.round(element.fontSize * Math.max(0.05, fontScale)),
      8,
      512,
    );
    const pad = textHasBackgroundPlate(element)
      ? textBackgroundPad(nextFontSize)
      : { x: 0, y: 0 };
    const originX = nextBounds.x + pad.x;
    const originY = nextBounds.y + pad.y;
    if (autoWidth) {
      const nextWidth = fittedAutoWidthTextBox(element.text, nextFontSize);
      return {
        ...element,
        fontSize: nextFontSize,
        x: originX,
        y: originY,
        width: nextWidth,
        autoWidth: true,
      };
    }
    return {
      ...element,
      fontSize: nextFontSize,
      x: originX,
      y: originY,
      width: Math.max(
        minTextBoxWidth(nextFontSize),
        element.width * Math.max(0.05, fontScale),
      ),
      autoWidth: false,
    };
  }

  if (element.kind === "shape") {
    const start = mapPoint({ x: element.x, y: element.y });
    const end = mapPoint({ x: element.endX, y: element.endY });
    // Lines/arrows scale stroke (and therefore the arrow head) with the box so
    // a shrunk arrow does not keep a giant tip on a tiny shaft.
    const strokeScale = isCurveableStrokeShape(element)
      ? Math.max(0.05, Math.min(Math.abs(scaleX), Math.abs(scaleY)))
      : 1;
    return {
      ...element,
      x: start.x,
      y: start.y,
      endX: end.x,
      endY: end.y,
      controls: element.controls.map(mapPoint),
      style: strokeScale === 1
        ? element.style
        : {
          ...element.style,
          strokeWidth: clamp(element.style.strokeWidth * strokeScale, 1, 80),
        },
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
  if (handle === "n" || handle === "s") return "ns-resize";
  if (handle === "e" || handle === "w") return "ew-resize";
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
    const content = textContentSize(element);
    if (!textHasBackgroundPlate(element)) {
      return {
        x: element.x,
        y: element.y,
        width: content.width,
        height: content.height,
      };
    }
    // Include the painted bubble so Trim edges / selection hug the plate.
    const pad = textBackgroundPad(element.fontSize);
    return {
      x: element.x - pad.x,
      y: element.y - pad.y,
      width: content.width + pad.x * 2,
      height: content.height + pad.y * 2,
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

/** CSS size of each Layers-panel thumbnail (matches `.screenshot-layer-preview`). */
export const LAYER_PREVIEW_SIZE = { width: 46, height: 34 } as const;

/**
 * Map content bounds into a fixed preview box (contain + center).
 * Used so shape/path/text layer thumbnails match the painted geometry.
 */
export function previewTransformForBounds(
  bounds: EditorRect,
  previewWidth: number = LAYER_PREVIEW_SIZE.width,
  previewHeight: number = LAYER_PREVIEW_SIZE.height,
  padding = 3,
): { scale: number; translateX: number; translateY: number } {
  const innerW = Math.max(1, previewWidth - padding * 2);
  const innerH = Math.max(1, previewHeight - padding * 2);
  const scale = Math.min(
    innerW / Math.max(1, bounds.width),
    innerH / Math.max(1, bounds.height),
  );
  const scaledW = bounds.width * scale;
  const scaledH = bounds.height * scale;
  return {
    scale,
    translateX: (previewWidth - scaledW) / 2 - bounds.x * scale,
    translateY: (previewHeight - scaledH) / 2 - bounds.y * scale,
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

function imageOrientedNaturalSize(
  element: Pick<EditorImageElement, "naturalWidth" | "naturalHeight" | "orientation">,
): { width: number; height: number } {
  return imageOrientationSwapsAxes(element.orientation)
    ? { width: element.naturalHeight, height: element.naturalWidth }
    : { width: element.naturalWidth, height: element.naturalHeight };
}

export function imageSizeAtWidth(
  element: EditorImageElement,
  width: number,
): { width: number; height: number } {
  const nextWidth = Math.max(1, Math.round(width));
  const natural = imageOrientedNaturalSize(element);
  return {
    width: nextWidth,
    height: Math.max(1, Math.round(nextWidth * natural.height / natural.width)),
  };
}

export function imageSizeAtHeight(
  element: EditorImageElement,
  height: number,
): { width: number; height: number } {
  const nextHeight = Math.max(1, Math.round(height));
  const natural = imageOrientedNaturalSize(element);
  return {
    width: Math.max(1, Math.round(nextHeight * natural.width / natural.height)),
    height: nextHeight,
  };
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value));
}
