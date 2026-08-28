import { invoke, isTauri } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";

import { CompressionPreview } from "./CompressionPreview";
import { sameSortedIds } from "./lib/editorPresence";
import { formatFileSize } from "./lib/format";
import {
  buildScreenshotEditorDraftPayload,
  collectDocumentImageSources,
  isScreenshotDocumentDirty,
  SCREENSHOT_EDITOR_DRAFT_CLOSE_FLUSH_MS,
  SCREENSHOT_EDITOR_DRAFT_SAVE_MS,
  type LoadedScreenshotEditorDraft,
} from "./lib/screenshotEditorDraft";
import {
  applyImageBackgroundEdit,
  brushRadiusInNaturalPixels,
  brushStrokeDirtyRect,
  DEFAULT_BRUSH_HARDNESS,
  DEFAULT_REMOVE_BG_BRUSH_SIZE,
  DEFAULT_WAND_TOLERANCE,
  documentPointToImagePixel,
  hitTestImageElement,
  imageDataToCanvas,
  imageToImageData,
  paintWandColorLoupe,
  removeBgBrushScreenDiameter,
  removeColorToTransparent,
  rgbaToCss,
  rgbaToHex,
  sampleImagePixel,
  strokeRemoveBackgroundBrush,
  wandLoupeScreenPosition,
  WAND_LOUPE_SIZE_PX,
  type RemoveBackgroundMode,
  type Rgba,
} from "./lib/imageBackground";
import {
  ALIGNMENT_SNAP_SCREEN_PX,
  applyTextStylePreset,
  arrowBendAmount,
  arrowChordLength,
  arrowDefaultMidHandle,
  arrowHeadTangentPoint,
  arrowHeadWingTips,
  arrowStarterControls,
  arrowVertices,
  arrowWithBend,
  boundedCropRect,
  cropDragAspectRatio,
  canvasOverflowEdges,
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
  duplicateScreenshotElement,
  elementBounds,
  LAYER_PREVIEW_SIZE,
  mergedLayerName,
  estimateCanvasExportBytes,
  expandDocumentForElement,
  imageDropExpandPadding,
  expandDocumentToFitBounds,
  curveStrokeHoverHint,
  hitTestArrowHandle,
  hitTestElement,
  insertArrowControl,
  previewExpandedCanvasRect,
  previewTransformForBounds,
  hitTestResizeHandle,
  imageDropGuideAtPoint,
  imageOrientationMatrix,
  imageSourceDisplaySize,
  imageSizeAtWidth,
  isCurveableStrokeShape,
  isSupportedImageFile,
  loadImageFile,
  outputDimensions,
  positionImportedImageAtEdge,
  removeArrowControl,
  reorderScreenshotLayers,
  resolveImageDropTarget,
  resizeBoundsFromHandle,
  resizeCursor,
  resizeDocumentCanvas,
  resizeElement,
  resizeHandlePoint,
  isResizeCornerHandle,
  scaleArrowStrokeForLength,
  snapResizedBounds,
  snapTranslatedBounds,
  stackDropLightFocusAtPoint,
  defaultTextBoxWidth,
  estimateTextWidth,
  fitAutoWidthTextElement,
  fittedAutoWidthTextBox,
  isAutoWidthText,
  TEXT_LINE_HEIGHT_RATIO,
  TEXT_OPTICAL_CENTER_NUDGE_RATIO,
  textBackgroundPad,
  textGlyphDrawY,
  textHasBackgroundPlate,
  textPresetPrefersCenter,
  textStylePreset,
  wrapTextLines,
  translateElement,
  canvasTrimMarginPreview,
  trimDocumentToContent,
  transformImageElement,
  visibleContentBounds,
  type AlignmentSnapGuide,
  type CanvasTrimMarginPreview,
  type ArrowHandle,
  type EditorImageElement,
  type ImageDropPlacement,
  type ImageTransformAction,
  type ImageSnapEdge,
  type LayerBlendMode,
  type LayerDropPlacement,
  type EditorPoint,
  type EditorRect,
  type EditorTextElement,
  type ElementStyle,
  type ResizeHandle,
  type ScreenshotDocument,
  type ScreenshotElement,
  type ScreenshotTool,
  type ShapeKind,
  type TextStylePreset,
} from "./lib/screenshotEditor";
import { CustomSelect } from "./CustomSelect";
import { NumberInput } from "./NumberInput";
import { RangeSlider } from "./RangeSlider";
import type { CaptureArtifact, EditorLayerPresence } from "./types";

type ExportFormat = "png" | "jpeg" | "webp";
type ExportSize = "original" | "75" | "50" | "custom";
/** Shared compress quality notches for JPEG, WebP, and PNG. */
type ScreenshotQuality = "55" | "70" | "85" | "92";
/** Matches the recording editor: preserve by default, compress with presets, or cap size. */
type ScreenshotQualityMode = "preserve" | "compress" | "maximum";
type ScreenshotFileSizeUnit = "kb" | "mb" | "gb";

type CachedImage = {
  image: HTMLImageElement | HTMLCanvasElement;
  status: "loading" | "loaded" | "error";
};

type EditorGesture =
  | {
    kind: "move";
    pointerId: number;
    origin: EditorPoint;
    element: ScreenshotElement;
    wasSelected: boolean;
    didMove: boolean;
    initialDocument: ScreenshotDocument;
  }
  | {
    kind: "resize";
    pointerId: number;
    handle: ResizeHandle;
    element: ScreenshotElement;
    initialBounds: EditorRect;
    currentBounds: EditorRect;
    initialDocument: ScreenshotDocument;
  }
  | {
    kind: "draw";
    pointerId: number;
    elementId: string;
    initialDocument: ScreenshotDocument;
  }
  | {
    kind: "crop";
    pointerId: number;
    origin: EditorPoint;
    /** Live Shift-lock snapshot; null when Shift is not held (or a preset is set). */
    shiftAspect: number | null;
    /** Last canvas-clamped crop, used to freeze ratio when Shift goes down. */
    lastRect: EditorRect | null;
  }
  | {
    kind: "arrow-handle";
    pointerId: number;
    handle: ArrowHandle;
    element: Extract<ScreenshotElement, { kind: "shape" }>;
    initialDocument: ScreenshotDocument;
  }
  | {
    kind: "remove-bg";
    pointerId: number;
    mode: "erase" | "restore";
    elementId: string;
    sourceBeforeEdit: string;
    initialDocument: ScreenshotDocument;
    workingData: ImageData;
    workingCanvas: HTMLCanvasElement;
    originalData: ImageData | null;
    radius: number;
    lastPixel: { x: number; y: number } | null;
    pendingPixel: { x: number; y: number } | null;
    changed: boolean;
  };

type RemoveBackgroundGesture = Extract<EditorGesture, { kind: "remove-bg" }>;

type PanGesture = {
  pointerId: number;
  clientX: number;
  clientY: number;
  originPanX: number;
  originPanY: number;
};

/** True when a keyboard event is Command (Mac) or Ctrl (Windows/Linux). */
function isPanModifierKey(event: KeyboardEvent): boolean {
  return event.key === "Meta"
    || event.key === "Control"
    || event.code === "MetaLeft"
    || event.code === "MetaRight"
    || event.code === "ControlLeft"
    || event.code === "ControlRight";
}

/** Canvas is "lost" when almost none of it intersects the viewport. */
function isCanvasMostlyOffscreen(
  viewport: DOMRectReadOnly,
  surface: DOMRectReadOnly,
): boolean {
  const overlapWidth = Math.max(
    0,
    Math.min(surface.right, viewport.right) - Math.max(surface.left, viewport.left),
  );
  const overlapHeight = Math.max(
    0,
    Math.min(surface.bottom, viewport.bottom) - Math.max(surface.top, viewport.top),
  );
  const overlapArea = overlapWidth * overlapHeight;
  if (overlapArea <= 0) return true;
  const surfaceArea = Math.max(1, surface.width * surface.height);
  // A thin sliver still counts as lost (Maps-style recenter cue).
  return overlapArea < Math.min(48 * 48, surfaceArea * 0.04);
}

type SavedScreenshotEdit = {
  artifact: CaptureArtifact;
  path: string;
  format: ExportFormat;
};

type LayerDropTarget = {
  id: string;
  placement: LayerDropPlacement;
};

type ImageDropGuide = {
  edge: ImageDropPlacement;
  target: EditorRect;
  point: EditorPoint;
  focus: EditorRect;
};

type DropToastAnchor = {
  left: number;
  top: number;
};

/** Live preview while a layer is dragged/resized past the canvas edge. */
type CanvasExpandPreview = {
  edges: ImageSnapEdge[];
  /** Expanded canvas in current document coordinates (may have negative origin). */
  rect: EditorRect;
  /** Element whose overflow is painted faded outside the current canvas. */
  element: ScreenshotElement;
  /** Pre-expand canvas size used to punch out the solid on-canvas region. */
  canvas: Pick<ScreenshotDocument, "width" | "height">;
};

type MagnifyGestureEvent = Event & {
  clientX?: number;
  clientY?: number;
  scale?: number;
};

const TOOL_ITEMS: Array<{ tool: ScreenshotTool; label: string; shortcut: string }> = [
  { tool: "select", label: "Select & move", shortcut: "V" },
  { tool: "crop", label: "Crop", shortcut: "C" },
  { tool: "text", label: "Text", shortcut: "T" },
  { tool: "rectangle", label: "Rectangle", shortcut: "R" },
  { tool: "ellipse", label: "Ellipse", shortcut: "O" },
  { tool: "line", label: "Line", shortcut: "L" },
  { tool: "arrow", label: "Arrow", shortcut: "A" },
  { tool: "pen", label: "Freehand", shortcut: "P" },
  { tool: "remove-bg", label: "Eraser", shortcut: "B" },
];

const TEXT_STYLE_ITEMS: Array<{ preset: TextStylePreset; label: string }> = [
  { preset: "standard", label: "Standard" },
  { preset: "rounded", label: "Rounded" },
  { preset: "outlined", label: "Outlined" },
  { preset: "mono", label: "Mono" },
  { preset: "box", label: "Box" },
  { preset: "mono-box", label: "Mono Box" },
  { preset: "rounded-box", label: "Rounded Box" },
];

/** Tools that draw closed or open vector shapes (not freehand). */
function isShapeDrawTool(tool: ScreenshotTool): boolean {
  return tool === "rectangle"
    || tool === "ellipse"
    || tool === "line"
    || tool === "arrow";
}

/**
 * Hit-test resize / curve handles on the selected annotation.
 * Curveable strokes prefer path handles so the mid control stays easy to grab
 * on thin lines (corner resize boxes sit near the stroke pad).
 */
function hitTestSelectedAnnotation(
  selected: ScreenshotElement,
  point: EditorPoint,
  interactionRadius: number,
): (
  | { kind: "resize"; handle: ResizeHandle; bounds: EditorRect }
  | { kind: "arrow-handle"; handle: ArrowHandle }
  | null
) {
  if (selected.locked || !selected.visible) return null;
  const bounds = elementBounds(selected);
  const handle = hitTestResizeHandle(
    bounds,
    point,
    interactionRadius,
    selected.kind === "text" ? "corners" : "all",
  );
  // Arrow endpoints sit near the dashed-box corners. Prefer those corners so
  // shrinking the box scales the whole arrow (shaft + head), not just the tip.
  if (handle && isResizeCornerHandle(handle)) {
    return { kind: "resize", handle, bounds };
  }
  if (selected.kind === "shape" && isCurveableStrokeShape(selected)) {
    const strokeHandle = hitTestArrowHandle(selected, point, interactionRadius);
    if (strokeHandle) return { kind: "arrow-handle", handle: strokeHandle };
  }
  if (handle) return { kind: "resize", handle, bounds };
  return null;
}

/** Body hit used when the active shape tool keeps manipulating its own shape. */
function hitTestSelectedShapeBody(
  selected: ScreenshotElement,
  point: EditorPoint,
  interactionRadius: number,
): boolean {
  if (
    selected.kind !== "shape"
    || selected.locked
    || !selected.visible
  ) {
    return false;
  }
  if (isCurveableStrokeShape(selected)) {
    const closest = closestPointOnArrow(selected, point);
    const pathHitRadius = Math.max(
      interactionRadius,
      selected.style.strokeWidth * 2 + interactionRadius * 0.6,
    );
    return closest.distance <= pathHitRadius;
  }
  return hitTestElement([selected], point, interactionRadius) !== null;
}

const REMOVE_BG_MODE_ITEMS: Array<{ mode: RemoveBackgroundMode; label: string }> = [
  { mode: "wand", label: "Wand" },
  { mode: "erase", label: "Erase" },
  { mode: "restore", label: "Restore" },
];

const COLOR_SWATCHES = [
  "#ff3b5c",
  "#ff8a22",
  "#ffd22e",
  "#36c96b",
  "#2d9cff",
  "#8b5cf6",
  "#111318",
  "#ffffff",
];

/** Solid canvas fill restored when "Solid background" is turned back on. */
const DEFAULT_CANVAS_BACKGROUND = "#f7f7f5";

/**
 * Shared compress presets. JPEG / WebP use encode quality; PNG maps the same
 * notches onto palette size plus compact packing.
 */
const SCREENSHOT_QUALITY_OPTIONS = [
  {
    value: "55",
    label: "Tiny",
    jpegDescription: "Smallest file with the most visible compression.",
    webpDescription: "Smallest lossy WebP with the most visible compression.",
    pngDescription: "Smallest PNG with the most visible compression.",
  },
  {
    value: "70",
    label: "Smaller",
    jpegDescription: "Very small file with more visible compression.",
    webpDescription: "Very small lossy WebP with more visible compression.",
    pngDescription: "Very small PNG with more visible compression.",
  },
  {
    value: "85",
    label: "Balanced",
    jpegDescription: "Good quality with a meaningfully smaller file.",
    webpDescription: "Good lossy WebP quality with a meaningfully smaller file.",
    pngDescription: "Good quality with a meaningfully smaller PNG.",
  },
  {
    value: "92",
    label: "High",
    jpegDescription: "Larger file with the least quality loss.",
    webpDescription: "Larger lossy WebP with the least quality loss.",
    pngDescription: "Larger PNG with the least quality loss.",
  },
] as const;

/** Keep in sync with `png_palette_colors_for_quality` in the Rust encoder. */
function pngMaxColorsForQuality(quality: ScreenshotQuality): number {
  switch (quality) {
    case "55":
      return 32;
    case "70":
      return 64;
    case "85":
      return 128;
    case "92":
      return 256;
  }
}

function screenshotQualityDescription(
  format: ExportFormat,
  option: (typeof SCREENSHOT_QUALITY_OPTIONS)[number],
): string {
  if (format === "webp") return option.webpDescription;
  if (format === "png") return option.pngDescription;
  return option.jpegDescription;
}

const SCREENSHOT_FILE_SIZE_UNIT_BYTES: Record<ScreenshotFileSizeUnit, number> = {
  kb: 1_000,
  mb: 1_000_000,
  gb: 1_000_000_000,
};
const MAX_SCREENSHOT_OUTPUT_DIMENSION = 16_384;
const MAX_SCREENSHOT_OUTPUT_PIXELS = 100_000_000;
const MIN_SCREENSHOT_ZOOM_PERCENT = 5;
const MAX_SCREENSHOT_ZOOM_PERCENT = 800;
const KEYBOARD_ZOOM_FACTOR = 1.25;
const WHEEL_ZOOM_SENSITIVITY = 0.002;
const SCREENSHOT_ZOOM_OPTIONS = [50, 100, 200] as const;

const LAYER_BLEND_MODE_OPTIONS: Array<{ value: LayerBlendMode; label: string }> = [
  { value: "source-over", label: "Normal" },
  { value: "multiply", label: "Multiply" },
  { value: "screen", label: "Screen" },
  { value: "overlay", label: "Overlay" },
  { value: "darken", label: "Darken" },
  { value: "lighten", label: "Lighten" },
];

function isFileTransfer(dataTransfer: DataTransfer): boolean {
  return Array.from(dataTransfer.types).includes("Files")
    || dataTransfer.files.length > 0;
}

function clampScreenshotZoomPercent(value: number): number {
  const clamped = Math.min(
    MAX_SCREENSHOT_ZOOM_PERCENT,
    Math.max(MIN_SCREENSHOT_ZOOM_PERCENT, value),
  );
  return Math.round(clamped * 10) / 10;
}

function screenshotZoomLabel(value: number): string {
  return `${Number.isInteger(value) ? value : value.toFixed(1)}%`;
}

/**
 * Map zoom percent ↔ continuous slider position on a log scale.
 * Linear 5–800% puts ~100% (typical “fills the window”) near the left edge;
 * log space keeps useful mid-range zooms near the middle of the track.
 */
const ZOOM_SLIDER_LOG_SPAN = Math.log(
  MAX_SCREENSHOT_ZOOM_PERCENT / MIN_SCREENSHOT_ZOOM_PERCENT,
);

function zoomPercentToSliderPosition(percent: number): number {
  const clamped = clampScreenshotZoomPercent(percent);
  return Math.log(clamped / MIN_SCREENSHOT_ZOOM_PERCENT) / ZOOM_SLIDER_LOG_SPAN;
}

function sliderPositionToZoomPercent(position: number): number {
  const t = Math.min(1, Math.max(0, position));
  return clampScreenshotZoomPercent(
    MIN_SCREENSHOT_ZOOM_PERCENT * Math.exp(t * ZOOM_SLIDER_LOG_SPAN),
  );
}

function wheelZoomFactor(
  deltaY: number,
  deltaMode: number,
  viewportHeight: number,
): number {
  const deltaUnit = deltaMode === 1
    ? 16
    : deltaMode === 2
      ? Math.max(1, viewportHeight)
      : 1;
  const pixelDelta = Math.min(240, Math.max(-240, deltaY * deltaUnit));
  return Math.exp(-pixelDelta * WHEEL_ZOOM_SENSITIVITY);
}

function query(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name);
}

function editorId(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  return `editor-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function replaceElement(
  document: ScreenshotDocument,
  elementId: string,
  replacement: ScreenshotElement,
): ScreenshotDocument {
  return {
    ...document,
    elements: document.elements.map((element) => (
      element.id === elementId ? replacement : element
    )),
  };
}

function fontFamily(element: Extract<ScreenshotElement, { kind: "text" }>): string {
  if (element.fontFamily === "serif") return "Georgia, 'Times New Roman', serif";
  if (element.fontFamily === "mono") return "'SFMono-Regular', Consolas, monospace";
  if (element.fontFamily === "rounded") {
    return "ui-rounded, 'SF Pro Rounded', 'Arial Rounded MT Bold', system-ui, sans-serif";
  }
  return "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";
}

/** Offscreen 2D context for measuring live text while typing (not the editor canvas). */
let textMetricsContext: CanvasRenderingContext2D | null | undefined;

function measureTextElementLine(element: EditorTextElement, line: string): number {
  if (textMetricsContext === undefined) {
    textMetricsContext = typeof document === "undefined"
      ? null
      : document.createElement("canvas").getContext("2d");
  }
  if (!textMetricsContext) return estimateTextWidth(line, element.fontSize);
  textMetricsContext.font = [
    element.italic ? "italic" : "",
    element.bold ? "700" : "400",
    `${element.fontSize}px`,
    fontFamily(element),
  ].filter(Boolean).join(" ");
  const width = textMetricsContext.measureText(line || " ").width;
  return Number.isFinite(width) && width > 0
    ? width
    : estimateTextWidth(line, element.fontSize);
}

function fitLiveText(element: EditorTextElement): EditorTextElement {
  return fitAutoWidthTextElement(
    element,
    (line) => measureTextElementLine(element, line),
  );
}

function textOutlineWidth(fontSize: number): number {
  return Math.max(1.5, fontSize * 0.08);
}

function textBackgroundRadius(
  element: Extract<ScreenshotElement, { kind: "text" }>,
  backgroundWidth: number,
  backgroundHeight: number,
): number {
  if (!element.roundedBackground) return 0;
  // Full capsule when the plate is wider than tall (the usual bubble label).
  return Math.min(backgroundWidth / 2, backgroundHeight / 2);
}

function drawSmoothPath(
  context: CanvasRenderingContext2D,
  points: EditorPoint[],
): void {
  if (points.length === 0) return;
  context.beginPath();
  context.moveTo(points[0].x, points[0].y);
  if (points.length === 1) {
    context.lineTo(points[0].x + 0.01, points[0].y + 0.01);
  } else if (points.length === 2) {
    context.lineTo(points[1].x, points[1].y);
  } else {
    for (let index = 1; index < points.length - 1; index += 1) {
      const midpoint = {
        x: (points[index].x + points[index + 1].x) / 2,
        y: (points[index].y + points[index + 1].y) / 2,
      };
      context.quadraticCurveTo(points[index].x, points[index].y, midpoint.x, midpoint.y);
    }
    context.lineTo(points.at(-1)!.x, points.at(-1)!.y);
  }
  context.stroke();
}

/** Stroke an arrow shaft: straight, single quadratic, or smooth multi-control. */
function strokeArrowPath(
  context: CanvasRenderingContext2D,
  vertices: EditorPoint[],
): void {
  if (vertices.length < 2) return;
  context.beginPath();
  context.moveTo(vertices[0].x, vertices[0].y);
  if (vertices.length === 2) {
    context.lineTo(vertices[1].x, vertices[1].y);
  } else if (vertices.length === 3) {
    context.quadraticCurveTo(
      vertices[1].x,
      vertices[1].y,
      vertices[2].x,
      vertices[2].y,
    );
  } else {
    for (let index = 1; index < vertices.length - 2; index += 1) {
      const midpoint = {
        x: (vertices[index].x + vertices[index + 1].x) / 2,
        y: (vertices[index].y + vertices[index + 1].y) / 2,
      };
      context.quadraticCurveTo(
        vertices[index].x,
        vertices[index].y,
        midpoint.x,
        midpoint.y,
      );
    }
    const last = vertices.length - 1;
    context.quadraticCurveTo(
      vertices[last - 1].x,
      vertices[last - 1].y,
      vertices[last].x,
      vertices[last].y,
    );
  }
  context.stroke();
}

/** Compact endpoint grip for line/arrow ends (kept smaller than full resize corners). */
function drawArrowEndpointHandle(
  context: CanvasRenderingContext2D,
  point: EditorPoint,
  unit: number,
  accentColor: string,
): void {
  const size = 5.5 * unit;
  context.save();
  context.globalAlpha = 0.88;
  context.fillStyle = accentColor;
  context.strokeStyle = "rgba(255, 255, 255, 0.92)";
  context.lineWidth = 1.15 * unit;
  context.fillRect(point.x - size / 2, point.y - size / 2, size, size);
  context.strokeRect(point.x - size / 2, point.y - size / 2, size, size);
  context.restore();
}

/**
 * Subtle free-control / mid-handle grip for bending lines and arrows.
 * Drawn smaller and slightly translucent so they read as edit affordances,
 * not primary chrome, especially right after placing a stroke.
 */
function drawArrowControlHandle(
  context: CanvasRenderingContext2D,
  point: EditorPoint,
  unit: number,
  accentColor: string,
  options?: { stemFrom?: EditorPoint },
): void {
  context.save();
  context.globalAlpha = 0.82;
  context.strokeStyle = accentColor;
  context.fillStyle = "rgba(255, 255, 255, 0.94)";
  context.lineWidth = 1.15 * unit;
  if (options?.stemFrom) {
    context.globalAlpha = 0.45;
    context.setLineDash([3 * unit, 3 * unit]);
    context.beginPath();
    context.moveTo(options.stemFrom.x, options.stemFrom.y);
    context.lineTo(point.x, point.y);
    context.stroke();
    context.setLineDash([]);
    context.globalAlpha = 0.82;
  }
  context.beginPath();
  context.arc(point.x, point.y, 4.5 * unit, 0, Math.PI * 2);
  context.fill();
  context.stroke();
  context.restore();
}

function arrowHead(
  context: CanvasRenderingContext2D,
  end: EditorPoint,
  tangent: EditorPoint,
  strokeWidth: number,
  shaftLength?: number,
): void {
  const [wingA, wingB] = arrowHeadWingTips(end, tangent, strokeWidth, shaftLength);
  context.beginPath();
  context.moveTo(end.x, end.y);
  context.lineTo(wingA.x, wingA.y);
  context.moveTo(end.x, end.y);
  context.lineTo(wingB.x, wingB.y);
  context.stroke();
}

function drawShape(
  context: CanvasRenderingContext2D,
  element: Extract<ScreenshotElement, { kind: "shape" }>,
): void {
  const { x, y, endX, endY, shape, style } = element;
  context.save();
  context.strokeStyle = style.color;
  context.fillStyle = style.fill ?? "transparent";
  context.lineWidth = style.strokeWidth;
  context.lineCap = "round";
  context.lineJoin = "round";

  if (shape === "rectangle" || shape === "ellipse") {
    const left = Math.min(x, endX);
    const top = Math.min(y, endY);
    const width = Math.abs(endX - x);
    const height = Math.abs(endY - y);
    context.beginPath();
    if (shape === "rectangle") {
      context.roundRect(left, top, width, height, Math.min(12, width / 6, height / 6));
    } else {
      context.ellipse(
        left + width / 2,
        top + height / 2,
        width / 2,
        height / 2,
        0,
        0,
        Math.PI * 2,
      );
    }
    if (style.fill) context.fill();
    context.stroke();
    context.restore();
    return;
  }

  if (isCurveableStrokeShape(element)) {
    strokeArrowPath(context, arrowVertices(element));
    if (shape === "arrow") {
      arrowHead(
        context,
        { x: endX, y: endY },
        arrowHeadTangentPoint(element),
        style.strokeWidth,
        arrowChordLength(element),
      );
    }
    context.restore();
    return;
  }

  context.beginPath();
  context.moveTo(x, y);
  context.lineTo(endX, endY);
  context.stroke();
  context.restore();
}

function drawText(
  context: CanvasRenderingContext2D,
  element: Extract<ScreenshotElement, { kind: "text" }>,
): void {
  const boxWidth = Math.max(element.fontSize * 0.5, element.width);
  const lineHeight = element.fontSize * TEXT_LINE_HEIGHT_RATIO;
  context.save();
  context.font = [
    element.italic ? "italic" : "",
    element.bold ? "700" : "400",
    `${element.fontSize}px`,
    fontFamily(element),
  ].filter(Boolean).join(" ");
  context.textAlign = element.align;
  const lines = wrapTextLines(
    element.text,
    boxWidth,
    element.fontSize,
    (line) => context.measureText(line || " ").width,
  );
  const contentHeight = Math.max(1, lines.length) * lineHeight;
  const anchorX = element.align === "center"
    ? element.x + boxWidth / 2
    : element.align === "right" ? element.x + boxWidth : element.x;
  if (textHasBackgroundPlate(element)) {
    context.fillStyle = element.background!;
    const pad = textBackgroundPad(element.fontSize);
    const backgroundX = element.x - pad.x;
    const backgroundY = element.y - pad.y;
    const backgroundWidth = boxWidth + pad.x * 2;
    const backgroundHeight = contentHeight + pad.y * 2;
    const backgroundRadius = textBackgroundRadius(
      element,
      backgroundWidth,
      backgroundHeight,
    );
    if (backgroundRadius > 0) {
      context.beginPath();
      context.roundRect(
        backgroundX,
        backgroundY,
        backgroundWidth,
        backgroundHeight,
        backgroundRadius,
      );
      context.fill();
    } else {
      context.fillRect(backgroundX, backgroundY, backgroundWidth, backgroundHeight);
    }
  }
  context.fillStyle = element.color;
  context.strokeStyle = element.color;
  context.lineWidth = textOutlineWidth(element.fontSize);
  context.lineJoin = "round";
  lines.forEach((line, index) => {
    const sample = line || " ";
    const draw = textGlyphDrawY(
      element.y,
      element.fontSize,
      index,
      context.measureText(sample),
    );
    context.textBaseline = draw.baseline;
    if (element.outlined) {
      context.strokeText(sample, anchorX, draw.y);
    } else {
      context.fillText(sample, anchorX, draw.y);
    }
  });
  context.restore();
}

/** Paint one oriented bitmap into its axis-aligned layer bounds. */
function paintImageElementSource(
  context: CanvasRenderingContext2D,
  element: EditorImageElement,
  source: CanvasImageSource,
): void {
  const matrix = imageOrientationMatrix(element.orientation);
  const sourceSize = imageSourceDisplaySize(element);
  context.save();
  context.translate(
    element.x + element.width / 2,
    element.y + element.height / 2,
  );
  context.transform(matrix.a, matrix.b, matrix.c, matrix.d, 0, 0);
  context.drawImage(
    source,
    -sourceSize.width / 2,
    -sourceSize.height / 2,
    sourceSize.width,
    sourceSize.height,
  );
  context.restore();
}

/** Paint a single layer into an existing context (caller owns alpha / transform). */
function paintScreenshotElement(
  context: CanvasRenderingContext2D,
  element: ScreenshotElement,
  imageCache: Map<string, CachedImage>,
): void {
  if (element.kind === "image") {
    const cached = imageCache.get(element.src);
    if (cached?.status === "loaded") {
      paintImageElementSource(context, element, cached.image);
    }
    return;
  }
  if (element.kind === "text") {
    drawText(context, element);
    return;
  }
  if (element.kind === "shape") {
    drawShape(context, element);
    return;
  }
  context.save();
  context.strokeStyle = element.style.color;
  context.lineWidth = element.style.strokeWidth;
  context.lineCap = "round";
  context.lineJoin = "round";
  drawSmoothPath(context, element.points);
  context.restore();
}

const EMPTY_LAYER_PREVIEW_IMAGE_CACHE = new Map<string, CachedImage>();

/**
 * Floating magnified sample for the remove-bg wand (crosshair stays on the pixel).
 * Position is fixed to client coordinates so pan/zoom of the canvas do not drift it.
 */
function WandColorLoupe({
  clientX,
  clientY,
  color,
  canvasRef,
}: {
  clientX: number;
  clientY: number;
  color: Rgba | null;
  canvasRef: RefObject<HTMLCanvasElement | null>;
}) {
  const { left, top } = wandLoupeScreenPosition(clientX, clientY);
  const colorCss = color ? rgbaToCss(color) : null;
  const colorHex = color ? rgbaToHex(color) : null;
  const transparent = color != null && color.a === 0;
  return (
    <div
      className="screenshot-wand-loupe"
      role="tooltip"
      aria-label={
        colorHex
          ? transparent
            ? "Sample color: transparent"
            : `Sample color ${colorHex}`
          : "Sample color preview"
      }
      style={{ left, top, width: WAND_LOUPE_SIZE_PX, height: WAND_LOUPE_SIZE_PX }}
    >
      <canvas
        ref={canvasRef}
        className="screenshot-wand-loupe-canvas"
        width={WAND_LOUPE_SIZE_PX}
        height={WAND_LOUPE_SIZE_PX}
        aria-hidden="true"
      />
      <div className="screenshot-wand-loupe-rim" aria-hidden="true" />
      {(colorCss || colorHex) && (
        <div className="screenshot-wand-loupe-meta">
          <span
            className={[
              "screenshot-wand-loupe-swatch",
              transparent ? "is-transparent" : "",
            ].filter(Boolean).join(" ")}
            style={transparent || !colorCss ? undefined : { background: colorCss }}
            aria-hidden="true"
          />
          <span className="screenshot-wand-loupe-hex">
            {transparent ? "empty" : (colorHex ?? "—")}
          </span>
        </div>
      )}
    </div>
  );
}

/**
 * Keep thin strokes readable in the 46×34 thumbnail without changing geometry
 * or color. Scale is applied after this, so we inflate strokeWidth in document
 * units when it would otherwise paint below ~1.35 CSS px.
 */
function withPreviewStrokeFloor(
  element: ScreenshotElement,
  scale: number,
  minStrokeCssPx = 1.35,
): ScreenshotElement {
  if (element.kind !== "shape" && element.kind !== "path") return element;
  if (scale <= 0) return element;
  const minDocStroke = minStrokeCssPx / scale;
  if (element.style.strokeWidth >= minDocStroke) return element;
  return {
    ...element,
    style: {
      ...element.style,
      strokeWidth: minDocStroke,
    },
  };
}

/**
 * Live thumbnail for non-image layers: paints the real shape/path/text
 * (color, curve, fill, stroke) into the Layers panel preview box.
 */
const AnnotationLayerPreview = memo(function AnnotationLayerPreview({
  element,
}: {
  element: Exclude<ScreenshotElement, EditorImageElement>;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const cssW = LAYER_PREVIEW_SIZE.width;
    const cssH = LAYER_PREVIEW_SIZE.height;
    const dpr = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;
    const pixelW = Math.max(1, Math.round(cssW * dpr));
    const pixelH = Math.max(1, Math.round(cssH * dpr));
    if (canvas.width !== pixelW) canvas.width = pixelW;
    if (canvas.height !== pixelH) canvas.height = pixelH;

    const context = canvas.getContext("2d");
    if (!context) return;

    context.setTransform(dpr, 0, 0, dpr, 0, 0);
    context.clearRect(0, 0, cssW, cssH);
    context.imageSmoothingEnabled = true;
    context.imageSmoothingQuality = "high";

    const bounds = elementBounds(element);
    const { scale, translateX, translateY } = previewTransformForBounds(bounds);
    const painted = withPreviewStrokeFloor(element, scale);

    context.save();
    context.globalAlpha = Math.max(0, Math.min(1, element.opacity / 100));
    context.translate(translateX, translateY);
    context.scale(scale, scale);
    paintScreenshotElement(context, painted, EMPTY_LAYER_PREVIEW_IMAGE_CACHE);
    context.restore();
  }, [element]);

  return (
    <canvas
      ref={canvasRef}
      className="screenshot-layer-preview-canvas"
      width={LAYER_PREVIEW_SIZE.width}
      height={LAYER_PREVIEW_SIZE.height}
      aria-hidden="true"
    />
  );
});

function renderScreenshot(
  context: CanvasRenderingContext2D,
  document: ScreenshotDocument,
  imageCache: Map<string, CachedImage>,
  hiddenElementId: string | null = null,
): void {
  context.clearRect(0, 0, document.width, document.height);
  if (document.background) {
    context.fillStyle = document.background;
    context.fillRect(0, 0, document.width, document.height);
  }
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = "high";
  for (const element of document.elements) {
    if (!element.visible || element.id === hiddenElementId) continue;
    context.save();
    context.globalAlpha = Math.max(0, Math.min(1, element.opacity / 100));
    context.globalCompositeOperation = element.blendMode;
    paintScreenshotElement(context, element, imageCache);
    context.restore();
  }
}

/** Paint a stack of layers (opacity + blend) in storage order onto a context. */
function paintLayerStack(
  context: CanvasRenderingContext2D,
  layers: readonly ScreenshotElement[],
  imageCache: Map<string, CachedImage>,
): void {
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = "high";
  for (const element of layers) {
    context.save();
    context.globalAlpha = Math.max(0, Math.min(1, element.opacity / 100));
    context.globalCompositeOperation = element.blendMode;
    paintScreenshotElement(context, element, imageCache);
    context.restore();
  }
}

/**
 * Rasterize layers (optionally over a solid canvas background) into a PNG data
 * URL covering the full document. Used by merge down / merge visible / flatten.
 */
function rasterizeLayersToImage(
  document: Pick<ScreenshotDocument, "width" | "height">,
  layers: readonly ScreenshotElement[],
  imageCache: Map<string, CachedImage>,
  background: string | null = null,
): { src: string; width: number; height: number } {
  const missing = layers
    .filter((element): element is EditorImageElement => element.kind === "image")
    .find((element) => imageCache.get(element.src)?.status !== "loaded");
  if (missing) {
    throw new Error(`${missing.name} has not finished loading.`);
  }
  const canvas = window.document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(document.width));
  canvas.height = Math.max(1, Math.round(document.height));
  const context = canvas.getContext("2d");
  if (!context) throw new Error("Canvas rendering is unavailable.");
  context.clearRect(0, 0, canvas.width, canvas.height);
  if (background) {
    context.fillStyle = background;
    context.fillRect(0, 0, canvas.width, canvas.height);
  }
  paintLayerStack(context, layers, imageCache);
  return {
    src: canvas.toDataURL("image/png"),
    width: canvas.width,
    height: canvas.height,
  };
}

function createMergedImageLayer(
  id: string,
  raster: { src: string; width: number; height: number },
  name: string,
  options: { locked?: boolean; source?: EditorImageElement["source"] } = {},
): EditorImageElement {
  return {
    id,
    kind: "image",
    source: options.source ?? "imported",
    src: raster.src,
    name,
    sourceArtifactId: null,
    x: 0,
    y: 0,
    width: raster.width,
    height: raster.height,
    naturalWidth: raster.width,
    naturalHeight: raster.height,
    locked: options.locked ?? false,
    visible: true,
    opacity: 100,
    blendMode: "source-over",
  };
}

/**
 * Faded preview of the parts of an element that sit outside the current canvas,
 * painted into an overlay sized to the post-release expand rect.
 */
function paintCanvasExpandOverflow(
  context: CanvasRenderingContext2D,
  preview: CanvasExpandPreview,
  imageCache: Map<string, CachedImage>,
  opacity = 0.42,
): void {
  const { rect, element, canvas } = preview;
  context.clearRect(0, 0, rect.width, rect.height);
  if (!element.visible) return;

  context.save();
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = "high";
  // Document → expand-overlay: expand origin may be negative when growing left/top.
  context.translate(-rect.x, -rect.y);
  context.globalAlpha = Math.max(0, Math.min(1, (element.opacity / 100) * opacity));
  context.globalCompositeOperation = element.blendMode;
  paintScreenshotElement(context, element, imageCache);
  context.restore();

  // Keep only the off-canvas remainder so it abuts the solid on-canvas paint.
  context.save();
  context.globalCompositeOperation = "destination-out";
  context.fillStyle = "#000";
  context.fillRect(-rect.x, -rect.y, canvas.width, canvas.height);
  context.restore();
}

function drawEditorOverlays(
  context: CanvasRenderingContext2D,
  document: ScreenshotDocument,
  selected: ScreenshotElement | null,
  crop: EditorRect | null,
  displayScale: number,
  accentColor: string,
  selectionBoundsOverride: EditorRect | null = null,
): void {
  const unit = 1 / Math.max(0.01, displayScale);
  if ((selected?.visible ?? false) || selectionBoundsOverride) {
    const bounds = selectionBoundsOverride
      ?? (selected ? elementBounds(selected) : null);
    if (bounds) {
      const curveable = selected?.kind === "shape" && isCurveableStrokeShape(selected);
      // Text labels scale as a sticker: corner grips only, no independent stretch.
      const cornerGripsOnly = curveable || selected?.kind === "text";
      context.save();
      // Lighter selection chrome so post-place curve grips stay the focus.
      context.globalAlpha = curveable ? 0.55 : 0.9;
      context.strokeStyle = accentColor;
      context.lineWidth = (curveable ? 1.25 : 2) * unit;
      context.setLineDash([6 * unit, 4 * unit]);
      context.strokeRect(bounds.x, bounds.y, bounds.width, bounds.height);
      context.setLineDash([]);
      // Corner grips scale the whole annotation (including arrow heads).
      // Mid-edge grips stay off lines/arrows so curve dots remain easy to grab,
      // and off text so the plate cannot stretch independently of the type.
      context.globalAlpha = 0.88;
      context.fillStyle = accentColor;
      context.strokeStyle = "rgba(255, 255, 255, 0.9)";
      context.lineWidth = 1.1 * unit;
      const grip = 5.5 * unit;
      const midX = bounds.x + bounds.width / 2;
      const midY = bounds.y + bounds.height / 2;
      const gripPoints = cornerGripsOnly
        ? [
          [bounds.x, bounds.y],
          [bounds.x + bounds.width, bounds.y],
          [bounds.x + bounds.width, bounds.y + bounds.height],
          [bounds.x, bounds.y + bounds.height],
        ]
        : [
          [bounds.x, bounds.y],
          [midX, bounds.y],
          [bounds.x + bounds.width, bounds.y],
          [bounds.x + bounds.width, midY],
          [bounds.x + bounds.width, bounds.y + bounds.height],
          [midX, bounds.y + bounds.height],
          [bounds.x, bounds.y + bounds.height],
          [bounds.x, midY],
        ];
      for (const point of gripPoints) {
        context.fillRect(
          point[0] - grip / 2,
          point[1] - grip / 2,
          grip,
          grip,
        );
        context.strokeRect(
          point[0] - grip / 2,
          point[1] - grip / 2,
          grip,
          grip,
        );
      }
      if (selected?.kind === "shape" && isCurveableStrokeShape(selected)) {
        context.globalAlpha = 1;
        const start = { x: selected.x, y: selected.y };
        const end = { x: selected.endX, y: selected.endY };
        drawArrowEndpointHandle(context, start, unit, accentColor);
        drawArrowEndpointHandle(context, end, unit, accentColor);
        if (selected.controls.length === 0) {
          // Three starter dots make local multi-point shaping discoverable.
          for (const control of arrowStarterControls(selected)) {
            drawArrowControlHandle(context, control, unit, accentColor);
          }
        } else {
          for (const control of selected.controls) {
            // Stem from chord midpoint toward each free control for visual cue.
            const stemFrom = arrowDefaultMidHandle(selected);
            drawArrowControlHandle(context, control, unit, accentColor, {
              stemFrom: selected.controls.length === 1 ? stemFrom : undefined,
            });
          }
        }
      }
      context.restore();
    }
  }

  if (crop) {
    context.save();
    context.fillStyle = "rgba(5, 6, 8, .64)";
    context.beginPath();
    context.rect(0, 0, document.width, document.height);
    context.rect(crop.x, crop.y, crop.width, crop.height);
    context.fill("evenodd");
    context.strokeStyle = "#ffffff";
    context.lineWidth = 2 * unit;
    context.setLineDash([8 * unit, 5 * unit]);
    context.strokeRect(crop.x, crop.y, crop.width, crop.height);
    context.setLineDash([]);
    context.fillStyle = "#111216";
    context.font = `700 ${12 * unit}px -apple-system, sans-serif`;
    const label = `${crop.width} × ${crop.height}`;
    const labelWidth = context.measureText(label).width + 14 * unit;
    context.fillRect(
      crop.x + crop.width / 2 - labelWidth / 2,
      crop.y + 8 * unit,
      labelWidth,
      24 * unit,
    );
    context.fillStyle = "#ffffff";
    context.textAlign = "center";
    context.textBaseline = "middle";
    context.fillText(
      label,
      crop.x + crop.width / 2,
      crop.y + 20 * unit,
    );
    context.restore();
  }
}

async function canvasPngBytes(canvas: HTMLCanvasElement): Promise<number[]> {
  const blob = await new Promise<Blob>((resolve, reject) => {
    canvas.toBlob((result) => {
      if (result) resolve(result);
      else reject(new Error("The edited image could not be encoded."));
    }, "image/png");
  });
  return Array.from(new Uint8Array(await blob.arrayBuffer()));
}

function screenshotOutputDimensions(
  document: Pick<ScreenshotDocument, "width" | "height">,
  size: ExportSize,
  customWidth: number,
  customHeight: number,
): { width: number; height: number } {
  if (size === "original") return { width: document.width, height: document.height };
  if (size === "custom") {
    return {
      width: Math.max(1, Math.min(MAX_SCREENSHOT_OUTPUT_DIMENSION, Math.round(customWidth))),
      height: Math.max(1, Math.min(MAX_SCREENSHOT_OUTPUT_DIMENSION, Math.round(customHeight))),
    };
  }
  return outputDimensions(
    document.width,
    document.height,
    Math.round(document.width * Number(size) / 100),
  );
}

function screenshotPathMatchesFormat(path: string | null, format: ExportFormat): boolean {
  if (!path) return false;
  const extension = path.split(/[\\/]/).at(-1)?.split(".").at(-1)?.toLowerCase();
  if (format === "jpeg") return extension === "jpg" || extension === "jpeg";
  return extension === format;
}

function screenshotFileStem(path: string): string {
  const filename = path.split(/[\\/]/).at(-1) || "Captures_screenshot";
  return filename.replace(/\.[^.]+$/, "") || "Captures_screenshot";
}

function screenshotEditedFileStem(stem: string): string {
  const trimmed = stem.trim();
  if (!trimmed) return "Captures_screenshot-edited";
  if (trimmed.endsWith("-edited") || trimmed.endsWith("-copy")) return trimmed;
  return `${trimmed}-edited`;
}

function screenshotParentDirectory(path: string): string {
  const separator = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (separator < 0) return ".";
  if (separator === 0) return path.slice(0, 1);
  return path.slice(0, separator);
}

function screenshotFilenameError(fileStem: string): string {
  const trimmed = fileStem.trim();
  const reserved = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i;
  const forbidden = '<>:"/\\|?*';
  const hasForbiddenCharacter = Array.from(trimmed).some((character) => (
    character.charCodeAt(0) < 32 || forbidden.includes(character)
  ));
  if (
    !trimmed
    || trimmed !== fileStem
    || trimmed === "."
    || trimmed === ".."
    || hasForbiddenCharacter
    || /[. ]$/.test(trimmed)
    || reserved.test(trimmed)
  ) {
    return "Enter a filename without folders or reserved characters.";
  }
  return "";
}

function screenshotFormatExtension(
  format: ExportFormat,
  sourcePath: string | null,
): string {
  if (format !== "jpeg") return format;
  return sourcePath?.toLowerCase().endsWith(".jpeg") ? "jpeg" : "jpg";
}

function screenshotDestinationPath(
  directory: string,
  fileStem: string,
  format: ExportFormat,
  sourcePath: string | null,
): string {
  const separator = directory.includes("\\") && !directory.includes("/") ? "\\" : "/";
  const base = directory.replace(/[\\/]+$/, "");
  const filename = `${fileStem}.${screenshotFormatExtension(format, sourcePath)}`;
  return base ? `${base}${separator}${filename}` : `${separator}${filename}`;
}

function formatScreenshotMaximumFileSizeInput(
  bytes: number,
  unit: ScreenshotFileSizeUnit,
): string {
  const value = bytes / SCREENSHOT_FILE_SIZE_UNIT_BYTES[unit];
  return Number(value.toPrecision(8)).toString();
}

/**
 * When nothing about the export changes pixels or codec vs the loaded capture,
 * show the known original file size instead of a browser re-encode estimate.
 */
function shouldUseOriginalFileSizeEstimate(
  artifact: CaptureArtifact,
  editorDocument: ScreenshotDocument,
  baselineDocument: ScreenshotDocument | null,
  exportFormat: ExportFormat,
  exportSize: ExportSize,
  qualityMode: ScreenshotQualityMode,
): boolean {
  if (qualityMode !== "preserve") return false;
  if (exportSize !== "original") return false;
  if (exportFormat === "jpeg") return false;
  if (!baselineDocument) return false;
  if (
    editorDocument.width !== artifact.width
    || editorDocument.height !== artifact.height
  ) {
    return false;
  }
  if (JSON.stringify(editorDocument) !== JSON.stringify(baselineDocument)) {
    return false;
  }
  if (artifact.path) {
    return screenshotPathMatchesFormat(artifact.path, exportFormat);
  }
  // Fresh captures are written as PNG when no path is available yet.
  return exportFormat === "png";
}

export function ScreenshotEditor() {
  const artifactId = query("artifact_id");
  const [artifact, setArtifact] = useState<CaptureArtifact | null>(null);
  const [editorDocument, setEditorDocument] = useState<ScreenshotDocument | null>(null);
  const documentRef = useRef<ScreenshotDocument | null>(null);
  const [undoStack, setUndoStack] = useState<ScreenshotDocument[]>([]);
  const [redoStack, setRedoStack] = useState<ScreenshotDocument[]>([]);
  const [tool, setTool] = useState<ScreenshotTool>("select");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editingTextId, setEditingTextId] = useState<string | null>(null);
  const [cropSelection, setCropSelection] = useState<EditorRect | null>(null);
  const [cropAspect, setCropAspect] = useState("free");
  const [removeBgMode, setRemoveBgMode] = useState<RemoveBackgroundMode>("wand");
  const [wandTolerance, setWandTolerance] = useState(DEFAULT_WAND_TOLERANCE);
  const [wandContiguous, setWandContiguous] = useState(true);
  const [removeBgBrushSize, setRemoveBgBrushSize] = useState(DEFAULT_REMOVE_BG_BRUSH_SIZE);
  const [removeBgBusy, setRemoveBgBusy] = useState(false);
  const [defaultStyle, setDefaultStyle] = useState<ElementStyle>({
    color: "#ff3b5c",
    fill: null,
    strokeWidth: 8,
  });
  const [defaultFontSize, setDefaultFontSize] = useState(48);
  const [defaultTextStyle, setDefaultTextStyle] = useState<TextStylePreset>("rounded-box");
  const [fitScale, setFitScale] = useState(1);
  const [zoomMode, setZoomMode] = useState<"fit" | "manual">("fit");
  const [zoom, setZoom] = useState(100);
  const [imageRevision, setImageRevision] = useState(0);
  const [dragActive, setDragActive] = useState(false);
  const [imageDropGuide, setImageDropGuide] = useState<ImageDropGuide | null>(null);
  const imageDropGuideRef = useRef<ImageDropGuide | null>(null);
  const [dropToastAnchor, setDropToastAnchor] = useState<DropToastAnchor | null>(null);
  const [draggedLayerId, setDraggedLayerId] = useState<string | null>(null);
  const [resizePreviewBounds, setResizePreviewBounds] = useState<EditorRect | null>(null);
  const [alignmentGuides, setAlignmentGuides] = useState<AlignmentSnapGuide[]>([]);
  const [canvasExpandPreview, setCanvasExpandPreview] = useState<CanvasExpandPreview | null>(null);
  const [trimEdgesHover, setTrimEdgesHover] = useState(false);
  const [canvasCursor, setCanvasCursor] = useState<string | undefined>(undefined);
  /**
   * Circular brush ring for erase/restore (size matches brush × zoom).
   * System cursors cannot grow past ~128px, so we hide the cursor and paint a ring.
   */
  const [brushCursor, setBrushCursor] = useState<{
    clientX: number;
    clientY: number;
    mode: "erase" | "restore";
  } | null>(null);
  /**
   * Wand hover loupe: magnified natural-image pixels + sampled color beside the crosshair.
   * Visibility is also gated on tool/mode so we never need an effect to clear it.
   */
  const [wandLoupe, setWandLoupe] = useState<{
    clientX: number;
    clientY: number;
    src: string;
    pixelX: number;
    pixelY: number;
    color: Rgba | null;
  } | null>(null);
  /** Floating tip while hovering a line/arrow path or its curve handles. */
  const [curveHoverTip, setCurveHoverTip] = useState<{
    text: string;
    clientX: number;
    clientY: number;
  } | null>(null);
  /** Command/Ctrl held — pan-ready grab cursor over the viewport. */
  const [panReady, setPanReady] = useState(false);
  const [panActive, setPanActive] = useState(false);
  /** Free view offset (CSS px) so the canvas can be dragged fully off-screen. */
  const [viewPan, setViewPan] = useState({ x: 0, y: 0 });
  const [canvasOffscreen, setCanvasOffscreen] = useState(false);
  const [layerDropTarget, setLayerDropTarget] = useState<LayerDropTarget | null>(null);
  /** Which layer row's settings popover is open (⋯ menu). */
  const [layerMenuId, setLayerMenuId] = useState<string | null>(null);
  /** Draft for an image-layer name edited directly in the Layers list. */
  const [layerRename, setLayerRename] = useState<{ id: string; value: string } | null>(null);
  const layerMenuRootRef = useRef<HTMLElement | null>(null);
  const layerMenuPanelRef = useRef<HTMLDivElement | null>(null);
  const layerMenuTriggerRefs = useRef(new Map<string, HTMLButtonElement>());
  const [layerMenuPlacement, setLayerMenuPlacement] = useState<{
    top: number | "auto";
    bottom: number | "auto";
    left: number;
    maxHeight: number;
  } | null>(null);
  const [exportFormat, setExportFormat] = useState<ExportFormat>("png");
  const [exportSize, setExportSize] = useState<ExportSize>("original");
  const [customExportWidth, setCustomExportWidth] = useState(1_920);
  const [customExportHeight, setCustomExportHeight] = useState(1_080);
  const [exportAspectLocked, setExportAspectLocked] = useState(true);
  const [jpegQuality, setJpegQuality] = useState<ScreenshotQuality>("92");
  const [qualityMode, setQualityMode] =
    useState<ScreenshotQualityMode>("preserve");
  const [maximumFileSize, setMaximumFileSize] = useState("10");
  const [maximumFileSizeUnit, setMaximumFileSizeUnit] =
    useState<ScreenshotFileSizeUnit>("mb");
  const [exportSettingsOpen, setExportSettingsOpen] = useState(false);
  const [filenameStem, setFilenameStem] = useState("");
  const [destinationDirectory, setDestinationDirectory] = useState("");
  const [estimatedBytes, setEstimatedBytes] = useState<number | null>(null);
  const [estimatePending, setEstimatePending] = useState(false);
  const [compressPreviewPending, setCompressPreviewPending] = useState(false);
  const [compressPreviewError, setCompressPreviewError] = useState("");
  const [compressPreviewAfterUrl, setCompressPreviewAfterUrl] = useState<string | null>(null);
  const [compressPreviewBeforeBytes, setCompressPreviewBeforeBytes] = useState<number | null>(null);
  const [compressPreviewAfterBytes, setCompressPreviewAfterBytes] = useState<number | null>(null);
  const compressPreviewUrlsRef = useRef<{ before: string | null; after: string | null }>({
    before: null,
    after: null,
  });
  // Monotonic id so an older in-flight preview encode cannot overwrite the
  // result of a newer one when responses arrive out of order.
  const compressPreviewRequestRef = useRef(0);
  const baselineDocumentRef = useRef<ScreenshotDocument | null>(null);
  const [busy, setBusy] = useState<"copying" | "saving" | null>(null);
  /** Transient success for copy/save — does not replace the stable export hint. */
  const [success, setSuccess] = useState<{ kind: "copy" | "save"; message: string } | null>(null);
  const [error, setError] = useState("");
  /** Original capture was deleted after the editor opened; the edit is still exportable. */
  const [sourceMissing, setSourceMissing] = useState(false);
  /** True when this session restored a disk draft from a previous editor close. */
  const [draftRestored, setDraftRestored] = useState(false);
  const [makeCopy, setMakeCopy] = useState(false);
  const [saved, setSaved] = useState<SavedScreenshotEdit | null>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const expandOverflowCanvasRef = useRef<HTMLCanvasElement>(null);
  const brushCursorElementRef = useRef<HTMLDivElement>(null);
  const brushCursorPositionRef = useRef({ clientX: 0, clientY: 0 });
  const inlineTextRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const imageCacheRef = useRef(new Map<string, CachedImage>());
  const successTimerRef = useRef<number | null>(null);
  /** Bumps to cancel in-flight debounced draft writes. */
  const draftSaveGenerationRef = useRef(0);
  /** Latest flush function for the close handler (stable listener, no re-subscribe). */
  const flushEditorDraftRef = useRef<() => Promise<void>>(async () => undefined);
  const objectUrlsRef = useRef(new Set<string>());
  const gestureRef = useRef<EditorGesture | null>(null);
  /** Live natural-resolution canvas drawn over the target layer during brush strokes. */
  const removeBgLiveRef = useRef<{
    elementId: string;
    canvas: HTMLCanvasElement;
  } | null>(null);
  /** Reused 1×1 canvas for wand hover color sampling (avoids alloc on every move). */
  const wandSampleCanvasRef = useRef<HTMLCanvasElement | null>(null);
  /** Magnified-pixel loupe canvas next to the wand crosshair. */
  const wandLoupeCanvasRef = useRef<HTMLCanvasElement>(null);
  const panGestureRef = useRef<PanGesture | null>(null);
  const modifierPanRef = useRef(false);
  const viewPanRef = useRef({ x: 0, y: 0 });
  const selectInlineTextRef = useRef(false);
  const suppressInlineTextBlurRef = useRef(false);
  const layerClipboardRef = useRef<{
    element: ScreenshotElement;
    pasteCount: number;
  } | null>(null);
  const dropDepthRef = useRef(0);
  const displayedZoomPercentRef = useRef(100);
  const zoomAnchorFrameRef = useRef<number | null>(null);
  const removeBgPreviewFrameRef = useRef<number | null>(null);
  const magnifyGestureRef = useRef<{
    initialZoomPercent: number;
    clientX: number;
    clientY: number;
  } | null>(null);

  const attachBrushCursor = useCallback((element: HTMLDivElement | null) => {
    brushCursorElementRef.current = element;
    if (!element) return;
    const { clientX, clientY } = brushCursorPositionRef.current;
    element.style.left = `${clientX}px`;
    element.style.top = `${clientY}px`;
  }, []);

  const refreshDropToastAnchor = useCallback(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const bounds = viewport.getBoundingClientRect();
    const next = {
      left: bounds.left + bounds.width / 2,
      top: bounds.top + 18,
    };
    setDropToastAnchor((current) => (
      current
      && Math.abs(current.left - next.left) < 0.5
      && Math.abs(current.top - next.top) < 0.5
        ? current
        : next
    ));
  }, []);

  useLayoutEffect(() => {
    if (!dragActive) return undefined;
    refreshDropToastAnchor();
    window.addEventListener("resize", refreshDropToastAnchor);
    const viewport = viewportRef.current;
    let observer: ResizeObserver | null = null;
    if (viewport && typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(refreshDropToastAnchor);
      observer.observe(viewport);
    }
    return () => {
      window.removeEventListener("resize", refreshDropToastAnchor);
      observer?.disconnect();
    };
  }, [dragActive, refreshDropToastAnchor]);

  const replaceDocument = useCallback((next: ScreenshotDocument) => {
    documentRef.current = next;
    setEditorDocument(next);
  }, []);

  const clearSuccess = useCallback(() => {
    if (successTimerRef.current !== null) {
      window.clearTimeout(successTimerRef.current);
      successTimerRef.current = null;
    }
    setSuccess(null);
  }, []);

  const showSuccess = useCallback((kind: "copy" | "save", message: string) => {
    if (successTimerRef.current !== null) {
      window.clearTimeout(successTimerRef.current);
    }
    setSuccess({ kind, message });
    successTimerRef.current = window.setTimeout(() => {
      setSuccess(null);
      successTimerRef.current = null;
    }, 4_000);
  }, []);

  useEffect(() => () => {
    if (successTimerRef.current !== null) {
      window.clearTimeout(successTimerRef.current);
    }
    if (zoomAnchorFrameRef.current !== null) {
      window.cancelAnimationFrame(zoomAnchorFrameRef.current);
    }
    if (removeBgPreviewFrameRef.current !== null) {
      window.cancelAnimationFrame(removeBgPreviewFrameRef.current);
    }
  }, []);

  // Position the per-layer settings popover beside the ⋯ trigger (over the
  // canvas, not stacked on the layer list). Fixed coords avoid clipping from
  // the scrollable layers pane; clamp so the card stays on-screen.
  useLayoutEffect(() => {
    if (!layerMenuId) return;
    const trigger = layerMenuTriggerRefs.current.get(layerMenuId);
    if (!trigger) return;
    const place = () => {
      const bounds = trigger.getBoundingClientRect();
      const menuWidth = 280;
      const menuHeight = Math.min(560, window.innerHeight - 16);
      const gap = 10;
      // Prefer left of the trigger so the panel sits beside the sidebar.
      let left = bounds.left - menuWidth - gap;
      if (left < 8) {
        left = Math.min(
          bounds.right + gap,
          Math.max(8, window.innerWidth - menuWidth - 8),
        );
      }
      // Align with the trigger row; shift up only when the card would overflow.
      let top = bounds.top;
      const maxTop = Math.max(8, window.innerHeight - menuHeight - 8);
      if (top > maxTop) top = maxTop;
      if (top < 8) top = 8;
      setLayerMenuPlacement({
        top,
        bottom: "auto",
        left,
        maxHeight: Math.max(160, window.innerHeight - top - 8),
      });
    };
    place();
    window.addEventListener("resize", place);
    // Capture scroll from the layer list and nested scroll parents.
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [layerMenuId]);

  // Close the per-layer settings popover on outside click or Escape.
  useEffect(() => {
    if (!layerMenuId) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (target && (
        layerMenuRootRef.current?.contains(target)
        || layerMenuPanelRef.current?.contains(target)
      )) return;
      setLayerMenuId(null);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        setLayerMenuId(null);
      }
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [layerMenuId]);

  const commitDocument = useCallback((next: ScreenshotDocument) => {
    const current = documentRef.current;
    if (!current || JSON.stringify(current) === JSON.stringify(next)) return;
    setUndoStack((stack) => [...stack.slice(-99), current]);
    setRedoStack([]);
    replaceDocument(next);
    setSaved(null);
    clearSuccess();
  }, [clearSuccess, replaceDocument]);

  const ensureImage = useCallback((src: string): CachedImage => {
    const existing = imageCacheRef.current.get(src);
    if (existing) return existing;
    const image = new Image();
    const cached: CachedImage = { image, status: "loading" };
    imageCacheRef.current.set(src, cached);
    // Custom capture protocol needs CORS for canvas export. blob:/data: object
    // URLs from dropped files are same-origin and fail if marked anonymous.
    if (!src.startsWith("blob:") && !src.startsWith("data:")) {
      image.crossOrigin = "anonymous";
    }
    image.onload = () => {
      cached.status = "loaded";
      setImageRevision((revision) => revision + 1);
    };
    image.onerror = () => {
      cached.status = "error";
      setError("One of the images in this edit could not be loaded.");
      setImageRevision((revision) => revision + 1);
    };
    image.src = src;
    return cached;
  }, []);

  /** Encode any document image URL to PNG bytes for draft persistence. */
  const pngBytesForSource = useCallback(async (src: string): Promise<number[]> => {
    if (src.startsWith("data:image/png;base64,")) {
      const binary = atob(src.slice("data:image/png;base64,".length));
      const bytes = new Array<number>(binary.length);
      for (let index = 0; index < binary.length; index += 1) {
        bytes[index] = binary.charCodeAt(index);
      }
      return bytes;
    }
    const cached = ensureImage(src);
    for (let attempt = 0; attempt < 120 && cached.status === "loading"; attempt += 1) {
      await new Promise((resolve) => window.setTimeout(resolve, 25));
    }
    if (cached.status !== "loaded") {
      throw new Error("An image layer could not be saved into the edit draft.");
    }
    // Cache may hold an Image or a working canvas (e.g. after remove-bg).
    const source = cached.image;
    const width = Math.max(
      1,
      source instanceof HTMLImageElement
        ? source.naturalWidth || source.width || 1
        : source.width || 1,
    );
    const height = Math.max(
      1,
      source instanceof HTMLImageElement
        ? source.naturalHeight || source.height || 1
        : source.height || 1,
    );
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (!context) throw new Error("The edit draft could not be encoded.");
    context.drawImage(source, 0, 0, width, height);
    return canvasPngBytes(canvas);
  }, [ensureImage]);

  const persistEditorDraft = useCallback(async (
    document: ScreenshotDocument,
    artifactKey: string,
  ): Promise<void> => {
    if (!isScreenshotDocumentDirty(document, baselineDocumentRef.current)) {
      await invoke("discard_screenshot_editor_draft", { artifactId: artifactKey });
      return;
    }
    const payload = await buildScreenshotEditorDraftPayload(
      artifactKey,
      document,
      pngBytesForSource,
    );
    await invoke("save_screenshot_editor_draft", {
      request: {
        artifact_id: payload.artifact_id,
        document: payload.document,
        assets: payload.assets,
        updated_at_ms: payload.updated_at_ms,
      },
    });
  }, [pngBytesForSource]);

  const flushEditorDraft = useCallback(async (): Promise<void> => {
    draftSaveGenerationRef.current += 1;
    const document = documentRef.current;
    const artifactKey = artifactId;
    if (!document || !artifactKey) return;
    try {
      await persistEditorDraft(document, artifactKey);
    } catch {
      // Best-effort on close; a failed draft should not block quitting the editor.
    }
  }, [artifactId, persistEditorDraft]);

  useLayoutEffect(() => {
    flushEditorDraftRef.current = flushEditorDraft;
  }, [flushEditorDraft]);

  const discardRestoredDraft = useCallback(() => {
    if (!artifact) return;
    const next = createScreenshotDocument(
      artifact.full_url,
      artifact.width,
      artifact.height,
      artifact.id,
    );
    ensureImage(artifact.full_url);
    baselineDocumentRef.current = next;
    setUndoStack([]);
    setRedoStack([]);
    replaceDocument(next);
    setDraftRestored(false);
    setSelectedId(null);
    setEditingTextId(null);
    clearSuccess();
    setError("");
    draftSaveGenerationRef.current += 1;
    void invoke("discard_screenshot_editor_draft", { artifactId: artifact.id })
      .catch(() => undefined);
  }, [artifact, clearSuccess, ensureImage, replaceDocument]);

  const editorPresenceId = artifactId ? `screenshot-editor-${artifactId}` : null;
  const lastEmittedPresenceRef = useRef<string[] | null>(null);

  const emitEditorPresence = useCallback((artifactIds: string[]) => {
    if (!editorPresenceId) return;
    if (
      lastEmittedPresenceRef.current
      && sameSortedIds(lastEmittedPresenceRef.current, artifactIds)
    ) {
      return;
    }
    lastEmittedPresenceRef.current = artifactIds;
    void Promise.resolve(emit<EditorLayerPresence>("editor-layers-changed", {
      editor_id: editorPresenceId,
      artifact_ids: artifactIds,
    })).catch(() => undefined);
  }, [editorPresenceId]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    const objectUrls = objectUrlsRef.current;
    void (async () => {
      if (!artifactId) throw new Error("No screenshot was selected.");
      unlisten = await listen<string>("artifact-removed", ({ payload }) => {
        if (payload !== artifactId) return;
        // The canvas still holds the edited image — copy/save remain available.
        setSourceMissing(true);
        setMakeCopy(true);
        setError("");
        clearSuccess();
      });
      const loaded = await invoke<CaptureArtifact | null>("get_artifact", { artifactId });
      if (!active) return;
      if (!loaded) throw new Error("The screenshot is no longer available.");
      const initialPath = loaded.path ?? await invoke<string>("default_screenshot_edit_path", {
        artifactId: loaded.id,
        format: "png",
      });
      if (!active) return;
      const baseline = createScreenshotDocument(
        loaded.full_url,
        loaded.width,
        loaded.height,
        loaded.id,
      );
      ensureImage(loaded.full_url);
      let working = baseline;
      let restoredDraft = false;
      try {
        const draft = await invoke<LoadedScreenshotEditorDraft | null>(
          "load_screenshot_editor_draft",
          { artifactId: loaded.id },
        );
        if (
          draft
          && draft.document
          && Array.isArray(draft.document.elements)
          && typeof draft.document.width === "number"
          && typeof draft.document.height === "number"
        ) {
          working = draft.document;
          restoredDraft = true;
          collectDocumentImageSources(working).forEach((src) => ensureImage(src));
        }
      } catch {
        // Missing or corrupt drafts fall back to a fresh document.
      }
      if (!active) return;
      setArtifact(loaded);
      baselineDocumentRef.current = baseline;
      replaceDocument(working);
      setDraftRestored(restoredDraft);
      setCustomExportWidth(loaded.width);
      setCustomExportHeight(loaded.height);
      setExportFormat("png");
      setExportSize("original");
      setQualityMode("preserve");
      setMakeCopy(!loaded.path);
      setFilenameStem(screenshotFileStem(initialPath));
      setDestinationDirectory(screenshotParentDirectory(initialPath));
      setDefaultFontSize(Math.max(24, Math.min(72, Math.round(Math.min(loaded.width, loaded.height) * 0.055))));
    })().catch((reason) => {
      if (active) setError(String(reason));
    });
    return () => {
      active = false;
      unlisten?.();
      objectUrls.forEach((url) => URL.revokeObjectURL(url));
      objectUrls.clear();
    };
  }, [artifactId, clearSuccess, ensureImage, replaceDocument]);

  // Autosave dirty documents so closing the window can restore the session later.
  useEffect(() => {
    if (!artifactId || !editorDocument) return;
    const generation = draftSaveGenerationRef.current + 1;
    draftSaveGenerationRef.current = generation;
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          await persistEditorDraft(editorDocument, artifactId);
        } catch {
          // Keep editing; the next change or close flush can retry.
        }
        if (generation !== draftSaveGenerationRef.current) return;
      })();
    }, SCREENSHOT_EDITOR_DRAFT_SAVE_MS);
    return () => window.clearTimeout(timer);
  }, [artifactId, editorDocument, persistEditorDraft]);

  // Best-effort draft flush before the native window is destroyed.
  //
  // Tauri always prevent_close()s when a JS onCloseRequested listener exists,
  // then the API wrapper awaits this handler and calls destroy() unless
  // preventDefault() was used. Editor windows must therefore grant
  // core:window:allow-destroy (see capabilities/editors.json); without it the
  // red traffic-light / title-bar close control looks dead while minimize and
  // zoom still work. A full dirty-draft encode (every image layer → PNG →
  // number[] IPC) can also hang long enough that close appears broken. Cap the
  // wait so the window always closes; debounced autosave already keeps most
  // sessions recoverable. Keep this effect dependent only on artifactId so we
  // do not unlisten/re-listen on every render (unlisten can strand close).
  useEffect(() => {
    if (!isTauri() || !artifactId) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void getCurrentWindow().onCloseRequested(async () => {
      if (!active) return;
      await Promise.race([
        flushEditorDraftRef.current(),
        new Promise<void>((resolve) => {
          window.setTimeout(resolve, SCREENSHOT_EDITOR_DRAFT_CLOSE_FLUSH_MS);
        }),
      ]);
      // Do not preventDefault — allow Tauri to destroy after the race settles.
    }).then((dispose) => {
      if (!active) {
        dispose();
        return;
      }
      unlisten = dispose;
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [artifactId]);

  // Keep mini-previews in sync with image layers still present in this editor.
  // Deleting a layer drops that capture; closing the window clears them all.
  useEffect(() => {
    if (!editorPresenceId) return;
    const artifactIds = editorDocument
      ? collectEditorSourceArtifactIds(editorDocument.elements)
      : [];
    emitEditorPresence(artifactIds);
  }, [editorDocument, editorPresenceId, emitEditorPresence]);

  useEffect(() => {
    if (!editorPresenceId) return;
    return () => {
      lastEmittedPresenceRef.current = null;
      void Promise.resolve(emit<EditorLayerPresence>("editor-layers-changed", {
        editor_id: editorPresenceId,
        artifact_ids: [],
      })).catch(() => undefined);
    };
  }, [editorPresenceId]);

  const selected = useMemo(() => (
    editorDocument?.elements.find((element) => element.id === selectedId) ?? null
  ), [editorDocument, selectedId]);

  /** True when visible layers leave empty margin (or overhang) on the canvas. */
  const canTrimEdges = useMemo(() => {
    if (!editorDocument) return false;
    const content = visibleContentBounds(editorDocument);
    if (!content) return false;
    return trimDocumentToContent(editorDocument) !== editorDocument;
  }, [editorDocument]);

  /** Hover preview: which current-canvas margins Trim edges would discard. */
  const trimEdgesPreview = useMemo((): CanvasTrimMarginPreview | null => {
    if (!trimEdgesHover || !canTrimEdges || !editorDocument) return null;
    return canvasTrimMarginPreview(editorDocument);
  }, [trimEdgesHover, canTrimEdges, editorDocument]);

  const editingText = editingTextId === selectedId && selected?.kind === "text"
    ? selected
    : null;

  const beginTextEditing = useCallback((elementId: string, selectAll = false) => {
    selectInlineTextRef.current = selectAll;
    setSelectedId(elementId);
    setEditingTextId(elementId);
    setTool("select");
    setCropSelection(null);
  }, []);

  const beginTextEditingFromPointerDown = useCallback((elementId: string, selectAll = false) => {
    suppressInlineTextBlurRef.current = true;
    const release = () => {
      window.removeEventListener("pointerup", release, true);
      window.removeEventListener("pointercancel", release, true);
      window.requestAnimationFrame(() => {
        suppressInlineTextBlurRef.current = false;
      });
    };
    window.addEventListener("pointerup", release, true);
    window.addEventListener("pointercancel", release, true);
    beginTextEditing(elementId, selectAll);
  }, [beginTextEditing]);

  useLayoutEffect(() => {
    if (!editingTextId) return;
    const focus = () => {
      const input = inlineTextRef.current;
      if (!input) return;
      input.focus({ preventScroll: true });
      if (selectInlineTextRef.current) input.select();
      else input.setSelectionRange(input.value.length, input.value.length);
    };
    focus();
    const frame = window.requestAnimationFrame(() => {
      focus();
      selectInlineTextRef.current = false;
    });
    return () => window.cancelAnimationFrame(frame);
  }, [editingTextId]);

  const displayScale = zoomMode === "fit" ? fitScale : zoom / 100;

  const inlineTextLayout = useMemo(() => {
    if (!editingText) return null;
    const contentHeight = elementBounds(editingText).height;
    const pad = textHasBackgroundPlate(editingText)
      ? textBackgroundPad(editingText.fontSize)
      : { x: 0, y: 0 };
    // Match the painted bubble (padding + layout box) so wrapping tracks canvas width.
    // Border is extra under border-box; include it so the content box matches `width`.
    const border = 2;
    const optical = editingText.fontSize * TEXT_OPTICAL_CENTER_NUDGE_RATIO * displayScale;
    const padY = pad.y * displayScale;
    return {
      left: (editingText.x - pad.x) * displayScale,
      top: (editingText.y - pad.y) * displayScale,
      width: Math.max(
        48,
        (editingText.width + pad.x * 2) * displayScale + border,
      ),
      height: Math.max(
        28,
        // elementBounds already includes pad when a plate is present.
        contentHeight * displayScale + border,
      ),
      padding: `${padY + optical}px ${pad.x * displayScale}px ${Math.max(0, padY - optical)}px`,
    };
  }, [displayScale, editingText]);

  useLayoutEffect(() => {
    displayedZoomPercentRef.current = displayScale * 100;
  }, [displayScale]);

  const setManualZoom = useCallback((
    requestedZoomPercent: number,
    clientPoint?: { clientX: number; clientY: number },
  ) => {
    if (!Number.isFinite(requestedZoomPercent)) return;
    const nextZoomPercent = clampScreenshotZoomPercent(requestedZoomPercent);
    const viewport = viewportRef.current;
    const canvas = canvasRef.current;
    let anchor: {
      clientX: number;
      clientY: number;
      xRatio: number;
      yRatio: number;
    } | null = null;

    if (viewport && canvas) {
      const viewportBounds = viewport.getBoundingClientRect();
      const canvasBounds = canvas.getBoundingClientRect();
      const clientX = clientPoint?.clientX
        ?? viewportBounds.left + viewportBounds.width / 2;
      const clientY = clientPoint?.clientY
        ?? viewportBounds.top + viewportBounds.height / 2;
      if (canvasBounds.width > 0 && canvasBounds.height > 0) {
        anchor = {
          clientX,
          clientY,
          xRatio: (clientX - canvasBounds.left) / canvasBounds.width,
          yRatio: (clientY - canvasBounds.top) / canvasBounds.height,
        };
      }
    }

    displayedZoomPercentRef.current = nextZoomPercent;
    setZoom(nextZoomPercent);
    setZoomMode("manual");

    if (!viewport || !canvas || !anchor) return;
    if (zoomAnchorFrameRef.current !== null) {
      window.cancelAnimationFrame(zoomAnchorFrameRef.current);
    }
    zoomAnchorFrameRef.current = window.requestAnimationFrame(() => {
      zoomAnchorFrameRef.current = null;
      const nextBounds = canvas.getBoundingClientRect();
      const nextClientX = nextBounds.left + nextBounds.width * anchor.xRatio;
      const nextClientY = nextBounds.top + nextBounds.height * anchor.yRatio;
      // Prefer scroll when the viewport overflows; otherwise nudge free pan so
      // zoom still stays under the pointer after a Command/Ctrl drag-pan.
      const dx = nextClientX - anchor.clientX;
      const dy = nextClientY - anchor.clientY;
      const prevLeft = viewport.scrollLeft;
      const prevTop = viewport.scrollTop;
      viewport.scrollLeft = prevLeft + dx;
      viewport.scrollTop = prevTop + dy;
      const scrolledX = viewport.scrollLeft - prevLeft;
      const scrolledY = viewport.scrollTop - prevTop;
      const residualX = dx - scrolledX;
      const residualY = dy - scrolledY;
      if (Math.abs(residualX) > 0.5 || Math.abs(residualY) > 0.5) {
        const nextPan = {
          x: viewPanRef.current.x - residualX,
          y: viewPanRef.current.y - residualY,
        };
        viewPanRef.current = nextPan;
        setViewPan(nextPan);
      }
    });
  }, []);

  const zoomBy = useCallback((
    factor: number,
    clientPoint?: { clientX: number; clientY: number },
  ) => {
    setManualZoom(displayedZoomPercentRef.current * factor, clientPoint);
  }, [setManualZoom]);

  const activateFitZoom = useCallback(() => {
    if (zoomAnchorFrameRef.current !== null) {
      window.cancelAnimationFrame(zoomAnchorFrameRef.current);
      zoomAnchorFrameRef.current = null;
    }
    viewPanRef.current = { x: 0, y: 0 };
    setViewPan({ x: 0, y: 0 });
    setZoomMode("fit");
  }, []);

  const canvasEditingTextId = editingText?.id ?? null;

  useLayoutEffect(() => {
    if (!editorDocument || !viewportRef.current) return;
    const viewport = viewportRef.current;
    const update = () => {
      const widthScale = Math.max(0.02, (viewport.clientWidth - 56) / editorDocument.width);
      const heightScale = Math.max(0.02, (viewport.clientHeight - 56) / editorDocument.height);
      setFitScale(Math.min(1, widthScale, heightScale));
    };
    update();
    if (typeof ResizeObserver !== "function") return;
    const observer = new ResizeObserver(update);
    observer.observe(viewport);
    return () => observer.disconnect();
  }, [editorDocument]);

  const paintEditorCanvas = useCallback(() => {
    if (!editorDocument || !canvasRef.current) return;
    editorDocument.elements
      .filter((element): element is EditorImageElement => element.kind === "image")
      .forEach((element) => {
        ensureImage(element.src);
        if (element.originalSrc) ensureImage(element.originalSrc);
      });
    const canvas = canvasRef.current;
    if (canvas.width !== editorDocument.width) canvas.width = editorDocument.width;
    if (canvas.height !== editorDocument.height) canvas.height = editorDocument.height;
    const context = canvas.getContext("2d");
    if (!context) return;
    const live = removeBgLiveRef.current;
    const hiddenElementId = live?.elementId ?? canvasEditingTextId;
    renderScreenshot(context, editorDocument, imageCacheRef.current, hiddenElementId);
    // Live erase/restore: draw the working natural-res canvas in place of the layer.
    if (live) {
      const liveElement = editorDocument.elements.find((element) => element.id === live.elementId);
      if (liveElement?.kind === "image" && liveElement.visible) {
        context.save();
        context.globalAlpha = Math.max(0, Math.min(1, liveElement.opacity / 100));
        context.globalCompositeOperation = liveElement.blendMode;
        paintImageElementSource(context, liveElement, live.canvas);
        context.restore();
      }
    }
    const accentColor = getComputedStyle(canvas)
      .getPropertyValue("--theme-accent")
      .trim() || "#ffffff";
    // While the inline text editor is open it provides its own focus chrome;
    // drawing the selection box too produces a second dashed highlight.
    drawEditorOverlays(
      context,
      editorDocument,
      canvasEditingTextId === null ? selected : null,
      cropSelection,
      displayScale,
      accentColor,
      canvasEditingTextId === null ? resizePreviewBounds : null,
    );
  }, [
    cropSelection,
    displayScale,
    canvasEditingTextId,
    editorDocument,
    resizePreviewBounds,
    ensureImage,
    selected,
  ]);

  useEffect(() => {
    paintEditorCanvas();
  }, [imageRevision, paintEditorCanvas]);

  // Paint the wand's magnified color loupe whenever the sample pixel moves.
  useLayoutEffect(() => {
    if (!wandLoupe) return;
    const canvas = wandLoupeCanvasRef.current;
    if (!canvas) return;
    const cached = imageCacheRef.current.get(wandLoupe.src);
    if (!cached || cached.status !== "loaded") return;
    paintWandColorLoupe(canvas, cached.image, wandLoupe.pixelX, wandLoupe.pixelY, {
      devicePixelRatio: typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1,
    });
  }, [wandLoupe, imageRevision]);

  // Faded off-canvas remainder of the active layer while expanding.
  useLayoutEffect(() => {
    const canvas = expandOverflowCanvasRef.current;
    if (!canvas || !canvasExpandPreview) return;
    const { rect, element } = canvasExpandPreview;
    if (element.kind === "image") ensureImage(element.src);
    if (canvas.width !== rect.width) canvas.width = Math.max(1, Math.ceil(rect.width));
    if (canvas.height !== rect.height) canvas.height = Math.max(1, Math.ceil(rect.height));
    const context = canvas.getContext("2d");
    if (!context) return;
    paintCanvasExpandOverflow(context, canvasExpandPreview, imageCacheRef.current);
  }, [canvasExpandPreview, ensureImage, imageRevision]);

  const undo = useCallback(() => {
    const current = documentRef.current;
    setUndoStack((stack) => {
      if (!current || stack.length === 0) return stack;
      const previous = stack.at(-1)!;
      setRedoStack((redo) => [current, ...redo].slice(0, 100));
      replaceDocument(previous);
      setSelectedId(null);
      setCropSelection(null);
      setSaved(null);
      return stack.slice(0, -1);
    });
  }, [replaceDocument]);

  const redo = useCallback(() => {
    const current = documentRef.current;
    setRedoStack((stack) => {
      if (!current || stack.length === 0) return stack;
      const next = stack[0];
      setUndoStack((undoHistory) => [...undoHistory.slice(-99), current]);
      replaceDocument(next);
      setSelectedId(null);
      setCropSelection(null);
      setSaved(null);
      return stack.slice(1);
    });
  }, [replaceDocument]);

  const deleteLayer = useCallback((elementId: string | null) => {
    const current = documentRef.current;
    const element = current?.elements.find(({ id }) => id === elementId);
    if (!current || !element || element.locked) return;
    commitDocument({
      ...current,
      elements: current.elements.filter(({ id }) => id !== elementId),
    });
    setSelectedId((currentId) => (currentId === elementId ? null : currentId));
    setLayerMenuId(null);
  }, [commitDocument]);

  const deleteSelected = useCallback(() => {
    deleteLayer(selectedId);
  }, [deleteLayer, selectedId]);

  const nudgeSelected = useCallback((deltaX: number, deltaY: number) => {
    const current = documentRef.current;
    const element = current?.elements.find(({ id }) => id === selectedId);
    if (!current || !element || element.locked) return;
    commitDocument(replaceElement(
      current,
      element.id,
      translateElement(element, deltaX, deltaY),
    ));
  }, [commitDocument, selectedId]);

  const duplicateLayer = useCallback((elementId: string | null) => {
    const current = documentRef.current;
    const index = current?.elements.findIndex(({ id }) => id === elementId) ?? -1;
    if (!current || index < 0) return false;
    const duplicate = duplicateScreenshotElement(current.elements[index], editorId());
    const elements = [...current.elements];
    elements.splice(index + 1, 0, duplicate);
    commitDocument({ ...current, elements });
    setSelectedId(duplicate.id);
    setEditingTextId(null);
    setTool("select");
    setLayerMenuId(null);
    return true;
  }, [commitDocument]);

  const duplicateSelected = useCallback(() => {
    return duplicateLayer(selectedId);
  }, [duplicateLayer, selectedId]);

  const mergeLayerDown = useCallback((elementId: string | null) => {
    const current = documentRef.current;
    if (!current || !elementId || !canMergeLayerDown(current.elements, elementId)) return false;
    const index = current.elements.findIndex(({ id }) => id === elementId);
    if (index <= 0) return false;
    const below = current.elements[index - 1];
    const selected = current.elements[index];
    try {
      const layers = [below, selected];
      const raster = rasterizeLayersToImage(current, layers, imageCacheRef.current);
      const merged = createMergedImageLayer(
        editorId(),
        raster,
        mergedLayerName(layers),
      );
      ensureImage(merged.src);
      commitDocument(applyMergeLayerDown(current, selected.id, merged));
      setSelectedId(merged.id);
      setEditingTextId(null);
      setTool("select");
      setImageRevision((revision) => revision + 1);
      setLayerMenuId(null);
      setError("");
      return true;
    } catch (reason) {
      setError(String(reason));
      return false;
    }
  }, [commitDocument, ensureImage]);

  const mergeVisibleLayers = useCallback(() => {
    const current = documentRef.current;
    if (!current || !canMergeVisibleLayers(current.elements)) return false;
    const layers = current.elements.filter((element) => element.visible);
    try {
      const raster = rasterizeLayersToImage(current, layers, imageCacheRef.current);
      const merged = createMergedImageLayer(editorId(), raster, "Merged");
      ensureImage(merged.src);
      commitDocument(applyMergeVisibleLayers(current, merged));
      setSelectedId(merged.id);
      setEditingTextId(null);
      setTool("select");
      setImageRevision((revision) => revision + 1);
      setLayerMenuId(null);
      setError("");
      return true;
    } catch (reason) {
      setError(String(reason));
      return false;
    }
  }, [commitDocument, ensureImage]);

  const flattenImage = useCallback(() => {
    const current = documentRef.current;
    if (!current || !canFlattenLayers(current.elements, current.background)) return false;
    const layers = current.elements.filter((element) => element.visible);
    try {
      const raster = rasterizeLayersToImage(
        current,
        layers,
        imageCacheRef.current,
        current.background,
      );
      const merged = createMergedImageLayer(editorId(), raster, "Flattened", {
        locked: true,
        source: "background",
      });
      ensureImage(merged.src);
      commitDocument(applyFlattenLayers(current, merged));
      setSelectedId(merged.id);
      setEditingTextId(null);
      setTool("select");
      setImageRevision((revision) => revision + 1);
      setLayerMenuId(null);
      setError("");
      return true;
    } catch (reason) {
      setError(String(reason));
      return false;
    }
  }, [commitDocument, ensureImage]);

  const copySelectedLayer = useCallback(() => {
    const current = documentRef.current;
    const element = current?.elements.find(({ id }) => id === selectedId);
    if (!element) return false;
    // Documents are updated immutably, so this object is a stable clipboard snapshot.
    layerClipboardRef.current = { element, pasteCount: 0 };
    return true;
  }, [selectedId]);

  const pasteLayer = useCallback(() => {
    const clipboard = layerClipboardRef.current;
    const current = documentRef.current;
    if (!clipboard || !current) return false;
    clipboard.pasteCount += 1;
    const duplicate = duplicateScreenshotElement(
      clipboard.element,
      editorId(),
      24 * clipboard.pasteCount,
    );
    const selectedIndex = current.elements.findIndex(({ id }) => id === selectedId);
    const insertionIndex = selectedIndex >= 0 ? selectedIndex + 1 : current.elements.length;
    const elements = [...current.elements];
    elements.splice(insertionIndex, 0, duplicate);
    commitDocument({ ...current, elements });
    setSelectedId(duplicate.id);
    setEditingTextId(null);
    setTool("select");
    return true;
  }, [commitDocument, selectedId]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const command = event.metaKey || event.ctrlKey;
      const target = event.target as HTMLElement | null;
      const editingField = target instanceof Element
        && target.matches("input, textarea, select, [contenteditable=true]");
      const interactiveTarget = target instanceof Element
        && target.matches("input, textarea, select, button, a, [contenteditable=true]");
      // Command (macOS) / Ctrl (Windows & Linux) hold enables click-drag pan.
      if (isPanModifierKey(event) && !interactiveTarget) {
        modifierPanRef.current = true;
        setPanReady(true);
        return;
      }
      if (
        command
        && (
          event.key === "+"
          || event.key === "="
          || event.code === "Equal"
          || event.code === "NumpadAdd"
        )
      ) {
        event.preventDefault();
        zoomBy(KEYBOARD_ZOOM_FACTOR);
        return;
      }
      if (
        command
        && (
          event.key === "-"
          || event.key === "_"
          || event.code === "Minus"
          || event.code === "NumpadSubtract"
        )
      ) {
        event.preventDefault();
        zoomBy(1 / KEYBOARD_ZOOM_FACTOR);
        return;
      }
      if (command && (event.key === "0" || event.code === "Numpad0")) {
        event.preventDefault();
        setManualZoom(100);
        return;
      }
      if (editingField) return;
      if (command && event.key.toLowerCase() === "c") {
        if (copySelectedLayer()) event.preventDefault();
        return;
      }
      if (command && event.key.toLowerCase() === "v") {
        if (pasteLayer()) event.preventDefault();
        return;
      }
      if (command && event.key.toLowerCase() === "d") {
        if (duplicateSelected()) event.preventDefault();
        return;
      }
      if (command && event.key.toLowerCase() === "z") {
        event.preventDefault();
        if (event.shiftKey) redo();
        else undo();
        return;
      }
      if (event.key === "Backspace" || event.key === "Delete") {
        event.preventDefault();
        deleteSelected();
        return;
      }
      if (event.key === "Escape") {
        setEditingTextId(null);
        setSelectedId(null);
        setCropSelection(null);
        return;
      }
      const multiplier = event.shiftKey ? 10 : 1;
      if (event.key === "ArrowLeft") nudgeSelected(-multiplier, 0);
      else if (event.key === "ArrowRight") nudgeSelected(multiplier, 0);
      else if (event.key === "ArrowUp") nudgeSelected(0, -multiplier);
      else if (event.key === "ArrowDown") nudgeSelected(0, multiplier);
      else if (!command && !event.altKey) {
        const match = TOOL_ITEMS.find(({ shortcut }) => shortcut.toLowerCase() === event.key.toLowerCase());
        if (match) {
          setEditingTextId(null);
          setTool(match.tool);
          if (match.tool !== "select") setSelectedId(null);
          setCropSelection(null);
        }
      }
    };
    const stopModifierPan = () => {
      modifierPanRef.current = false;
      setPanReady(false);
    };
    const onKeyUp = (event: KeyboardEvent) => {
      if (isPanModifierKey(event) || (!event.metaKey && !event.ctrlKey)) {
        stopModifierPan();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", stopModifierPan);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", stopModifierPan);
    };
  }, [
    copySelectedLayer,
    deleteSelected,
    duplicateSelected,
    nudgeSelected,
    pasteLayer,
    redo,
    setManualZoom,
    undo,
    zoomBy,
  ]);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (!editorDocument || !viewport) return;

    const eventPoint = (event: MagnifyGestureEvent) => {
      const bounds = viewport.getBoundingClientRect();
      const clientX = event.clientX;
      const clientY = event.clientY;
      return {
        clientX: typeof clientX === "number" && Number.isFinite(clientX)
          ? clientX
          : bounds.left + bounds.width / 2,
        clientY: typeof clientY === "number" && Number.isFinite(clientY)
          ? clientY
          : bounds.top + bounds.height / 2,
      };
    };

    const onWheel = (event: WheelEvent) => {
      if ((!event.ctrlKey && !event.metaKey) || event.deltaY === 0) return;
      event.preventDefault();
      if (magnifyGestureRef.current) return;
      zoomBy(
        wheelZoomFactor(event.deltaY, event.deltaMode, viewport.clientHeight),
        { clientX: event.clientX, clientY: event.clientY },
      );
    };

    const onGestureStart = (event: Event) => {
      event.preventDefault();
      const point = eventPoint(event as MagnifyGestureEvent);
      magnifyGestureRef.current = {
        initialZoomPercent: displayedZoomPercentRef.current,
        ...point,
      };
    };

    const onGestureChange = (event: Event) => {
      const gesture = magnifyGestureRef.current;
      if (!gesture) return;
      event.preventDefault();
      const scale = (event as MagnifyGestureEvent).scale;
      if (typeof scale !== "number" || !Number.isFinite(scale) || scale <= 0) return;
      setManualZoom(gesture.initialZoomPercent * scale, {
        clientX: gesture.clientX,
        clientY: gesture.clientY,
      });
    };

    const onGestureEnd = (event: Event) => {
      if (!magnifyGestureRef.current) return;
      event.preventDefault();
      magnifyGestureRef.current = null;
    };

    viewport.addEventListener("wheel", onWheel, { passive: false });
    viewport.addEventListener("gesturestart", onGestureStart, { passive: false });
    viewport.addEventListener("gesturechange", onGestureChange, { passive: false });
    viewport.addEventListener("gestureend", onGestureEnd, { passive: false });
    viewport.addEventListener("gesturecancel", onGestureEnd, { passive: false });
    return () => {
      viewport.removeEventListener("wheel", onWheel);
      viewport.removeEventListener("gesturestart", onGestureStart);
      viewport.removeEventListener("gesturechange", onGestureChange);
      viewport.removeEventListener("gestureend", onGestureEnd);
      viewport.removeEventListener("gesturecancel", onGestureEnd);
      magnifyGestureRef.current = null;
    };
  }, [editorDocument, setManualZoom, zoomBy]);

  const startPanPointer = (event: React.PointerEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement | null;
    if (target?.closest("input, textarea, select, button, [contenteditable=true]")) return;
    // Pan from anywhere on the viewport — canvas, chrome, or layer hit targets —
    // while Command/Ctrl is held (or middle mouse). Capture phase wins over tools.
    const modifierPan = modifierPanRef.current || event.metaKey || event.ctrlKey;
    if ((event.button !== 0 || !modifierPan) && event.button !== 1) return;
    const viewport = event.currentTarget;
    panGestureRef.current = {
      pointerId: event.pointerId,
      clientX: event.clientX,
      clientY: event.clientY,
      originPanX: viewPanRef.current.x,
      originPanY: viewPanRef.current.y,
    };
    setPanActive(true);
    event.preventDefault();
    event.stopPropagation();
    if (typeof viewport.setPointerCapture === "function") {
      viewport.setPointerCapture(event.pointerId);
    }
  };

  const movePanPointer = (event: React.PointerEvent<HTMLDivElement>) => {
    const gesture = panGestureRef.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    const next = {
      x: gesture.originPanX + (event.clientX - gesture.clientX),
      y: gesture.originPanY + (event.clientY - gesture.clientY),
    };
    viewPanRef.current = next;
    setViewPan(next);
    event.preventDefault();
    event.stopPropagation();
  };

  const finishPanPointer = (event: React.PointerEvent<HTMLDivElement>) => {
    const gesture = panGestureRef.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    panGestureRef.current = null;
    setPanActive(false);
    event.preventDefault();
    event.stopPropagation();
    if (
      typeof event.currentTarget.hasPointerCapture === "function"
      && event.currentTarget.hasPointerCapture(event.pointerId)
    ) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };

  const recenterCanvas = useCallback(() => {
    viewPanRef.current = { x: 0, y: 0 };
    setViewPan({ x: 0, y: 0 });
    const viewport = viewportRef.current;
    if (viewport) {
      const maxLeft = Math.max(0, viewport.scrollWidth - viewport.clientWidth);
      const maxTop = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
      viewport.scrollLeft = maxLeft / 2;
      viewport.scrollTop = maxTop / 2;
    }
    setCanvasOffscreen(false);
  }, []);

  // Fade in Recenter when free pan (or scroll) leaves almost none of the canvas visible.
  useEffect(() => {
    const viewport = viewportRef.current;
    const surface = surfaceRef.current;
    if (!viewport || !surface || !editorDocument) {
      setCanvasOffscreen(false);
      return undefined;
    }

    const update = () => {
      setCanvasOffscreen(isCanvasMostlyOffscreen(
        viewport.getBoundingClientRect(),
        surface.getBoundingClientRect(),
      ));
    };

    update();
    viewport.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    let observer: ResizeObserver | null = null;
    if (typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(update);
      observer.observe(viewport);
      observer.observe(surface);
    }
    return () => {
      viewport.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
      observer?.disconnect();
    };
  }, [editorDocument, displayScale, viewPan]);

  /** Map client coordinates into document space (may be outside the canvas). */
  const clientToDocumentPoint = (clientX: number, clientY: number): EditorPoint => {
    const canvas = canvasRef.current;
    if (!canvas) return { x: 0, y: 0 };
    const bounds = canvas.getBoundingClientRect();
    return {
      x: (clientX - bounds.left) * canvas.width / Math.max(1, bounds.width),
      y: (clientY - bounds.top) * canvas.height / Math.max(1, bounds.height),
    };
  };

  const canvasPoint = (
    event: Pick<React.PointerEvent, "clientX" | "clientY">,
  ): EditorPoint => clientToDocumentPoint(event.clientX, event.clientY);

  /** Move an already-visible brush ring without rerendering the full editor. */
  const showBrushCursor = (
    clientX: number,
    clientY: number,
    mode: "erase" | "restore",
  ) => {
    brushCursorPositionRef.current = { clientX, clientY };
    const cursor = brushCursorElementRef.current;
    if (cursor) {
      cursor.style.left = `${clientX}px`;
      cursor.style.top = `${clientY}px`;
    }
    if (!brushCursor || brushCursor.mode !== mode) {
      setBrushCursor({ clientX, clientY, mode });
    }
  };

  /**
   * Erase/restore: hide the system cursor and show a ring sized to the brush.
   * Wand: crosshair + magnified color loupe beside the sample pixel.
   */
  const syncRemoveBgHoverCursor = (
    event: Pick<React.PointerEvent, "clientX" | "clientY">,
    point: EditorPoint,
    options?: { mode?: "erase" | "restore"; forceOverImage?: boolean },
  ) => {
    if (panActive || panReady) {
      setBrushCursor(null);
      setWandLoupe(null);
      return;
    }
    const mode = options?.mode ?? (
      removeBgMode === "restore" ? "restore" : removeBgMode === "erase" ? "erase" : null
    );
    if (removeBgMode === "wand" || mode == null) {
      setBrushCursor(null);
      const current = documentRef.current;
      const image = current ? hitTestImageElement(current.elements, point) : null;
      if (!image) {
        setCanvasCursor("not-allowed");
        setWandLoupe(null);
        return;
      }
      const pixel = documentPointToImagePixel(image, point);
      if (!pixel) {
        setCanvasCursor("not-allowed");
        setWandLoupe(null);
        return;
      }
      setCanvasCursor("crosshair");
      const cached = imageCacheRef.current.get(image.src);
      let color: Rgba | null = null;
      if (cached?.status === "loaded") {
        if (!wandSampleCanvasRef.current) {
          wandSampleCanvasRef.current = document.createElement("canvas");
        }
        color = sampleImagePixel(
          cached.image,
          pixel.x,
          pixel.y,
          wandSampleCanvasRef.current,
        );
      }
      setWandLoupe({
        clientX: event.clientX,
        clientY: event.clientY,
        src: image.src,
        pixelX: pixel.x,
        pixelY: pixel.y,
        color,
      });
      return;
    }
    setWandLoupe(null);
    const current = documentRef.current;
    const overImage = options?.forceOverImage
      || Boolean(current && hitTestImageElement(current.elements, point));
    if (overImage) {
      setCanvasCursor("none");
      showBrushCursor(event.clientX, event.clientY, mode);
    } else {
      setCanvasCursor("not-allowed");
      setBrushCursor(null);
    }
  };

  const capturePointerTarget = (
    target: EventTarget & { setPointerCapture?: (pointerId: number) => void },
    pointerId: number,
  ) => {
    if (typeof target.setPointerCapture === "function") {
      target.setPointerCapture(pointerId);
    }
  };

  const releasePointerTarget = (
    target: EventTarget & {
      hasPointerCapture?: (pointerId: number) => boolean;
      releasePointerCapture?: (pointerId: number) => void;
    },
    pointerId: number,
  ) => {
    if (
      typeof target.hasPointerCapture === "function"
      && typeof target.releasePointerCapture === "function"
      && target.hasPointerCapture(pointerId)
    ) {
      target.releasePointerCapture(pointerId);
    }
  };

  /** Apply the latest queued brush point and repaint once for this animation frame. */
  const flushRemoveBgBrushPreview = (): RemoveBackgroundGesture | null => {
    const gesture = gestureRef.current;
    if (!gesture || gesture.kind !== "remove-bg" || !gesture.pendingPixel) {
      return gesture?.kind === "remove-bg" ? gesture : null;
    }
    const from = gesture.lastPixel ?? gesture.pendingPixel;
    const to = gesture.pendingPixel;
    const stamped = strokeRemoveBackgroundBrush(
      gesture.workingData,
      from.x,
      from.y,
      to.x,
      to.y,
      gesture.radius,
      gesture.mode,
      gesture.originalData,
      DEFAULT_BRUSH_HARDNESS,
    );
    const next: RemoveBackgroundGesture = {
      ...gesture,
      lastPixel: to,
      pendingPixel: null,
      changed: gesture.changed || stamped > 0,
    };
    gestureRef.current = next;
    if (stamped > 0) {
      imageDataToCanvas(
        gesture.workingData,
        gesture.workingCanvas,
        brushStrokeDirtyRect(
          gesture.workingData.width,
          gesture.workingData.height,
          from.x,
          from.y,
          to.x,
          to.y,
          gesture.radius,
        ),
      );
      paintEditorCanvas();
    }
    return next;
  };

  /** Coalesce high-frequency pointer events into at most one bitmap paint per frame. */
  const scheduleRemoveBgBrushPreview = () => {
    if (removeBgPreviewFrameRef.current !== null) return;
    removeBgPreviewFrameRef.current = window.requestAnimationFrame(() => {
      removeBgPreviewFrameRef.current = null;
      flushRemoveBgBrushPreview();
    });
  };

  /**
   * Begin an edit gesture at a document-space point. The capture target may be
   * the canvas or the viewport chrome (so drawing can start outside the image).
   */
  const startPointerAt = (
    event: React.PointerEvent<Element>,
    point: EditorPoint,
  ) => {
    const current = documentRef.current;
    if (!current || event.button !== 0) return;
    const interactionRadius = 10 / Math.max(0.01, displayScale);
    if (tool !== "text") setEditingTextId(null);
    setError("");
    clearSuccess();
    setSaved(null);

    if (tool === "remove-bg") {
      const image = hitTestImageElement(current.elements, point);
      if (!image) {
        setError("Click an image layer to remove or restore its background.");
        setCanvasCursor("not-allowed");
        setBrushCursor(null);
        setWandLoupe(null);
        return;
      }
      setSelectedId(image.id);
      if (removeBgMode === "wand") {
        applyWandRemoveBackground(image, point, current);
        return;
      }
      beginRemoveBgBrush(event, image, point, removeBgMode, current);
      return;
    }

    if (tool === "select") {
      const selectedElement = selectedId
        ? current.elements.find((element) => element.id === selectedId) ?? null
        : null;
      if (selectedElement) {
        const annotationHit = hitTestSelectedAnnotation(
          selectedElement,
          point,
          interactionRadius,
        );
        if (annotationHit?.kind === "resize") {
          gestureRef.current = {
            kind: "resize",
            pointerId: event.pointerId,
            handle: annotationHit.handle,
            element: selectedElement,
            initialBounds: annotationHit.bounds,
            currentBounds: annotationHit.bounds,
            initialDocument: current,
          };
          setResizePreviewBounds(annotationHit.bounds);
          setCanvasCursor(resizeCursor(annotationHit.handle));
          capturePointerTarget(event.currentTarget, event.pointerId);
          return;
        }
        if (
          annotationHit?.kind === "arrow-handle"
          && selectedElement.kind === "shape"
        ) {
          gestureRef.current = {
            kind: "arrow-handle",
            pointerId: event.pointerId,
            handle: annotationHit.handle,
            element: selectedElement,
            initialDocument: current,
          };
          setCanvasCursor("grabbing");
          setCurveHoverTip(null);
          capturePointerTarget(event.currentTarget, event.pointerId);
          return;
        }
      }

      const element = hitTestElement(current.elements, point, interactionRadius);
      setSelectedId(element?.id ?? null);
      setCurveHoverTip(null);
      if (element) {
        gestureRef.current = {
          kind: "move",
          pointerId: event.pointerId,
          origin: point,
          element,
          wasSelected: element.id === selectedId,
          didMove: false,
          initialDocument: current,
        };
        setCanvasCursor("move");
        capturePointerTarget(event.currentTarget, event.pointerId);
      } else {
        setCanvasCursor(undefined);
      }
      return;
    }

    if (tool === "crop") {
      setSelectedId(null);
      const next = cropDragAspectRatio({
        preset: cropAspect,
        shiftKey: event.shiftKey,
        origin: point,
        current: point,
        bounds: current,
        shiftAspect: null,
        liveRect: null,
      });
      const rect = boundedCropRect(point, point, current, next.aspectRatio);
      setCropSelection(rect);
      gestureRef.current = {
        kind: "crop",
        pointerId: event.pointerId,
        origin: point,
        shiftAspect: next.shiftAspect,
        lastRect: rect,
      };
      capturePointerTarget(event.currentTarget, event.pointerId);
      return;
    }

    if (tool === "text") {
      const existing = hitTestElement(current.elements, point, interactionRadius);
      if (existing?.kind === "text") {
        beginTextEditingFromPointerDown(existing.id);
        return;
      }
      // Keep the coming textarea focused; otherwise the canvas click steals it.
      event.preventDefault();
      const styled = applyTextStylePreset({
        id: editorId(),
        kind: "text",
        x: point.x,
        y: point.y,
        text: "Text",
        fontSize: defaultFontSize,
        width: defaultTextBoxWidth(defaultFontSize),
        autoWidth: true,
        fontFamily: "sans",
        bold: false,
        italic: false,
        // Bubble / plate styles look off when left-aligned in a wider box.
        align: textPresetPrefersCenter(defaultTextStyle) ? "center" : "left",
        color: defaultStyle.color,
        background: null,
        outlined: false,
        roundedBackground: false,
        locked: false,
        visible: true,
        opacity: 100,
        blendMode: "source-over",
      }, defaultTextStyle);
      const element = {
        ...styled,
        autoWidth: true as const,
        width: fittedAutoWidthTextBox(
          styled.text,
          styled.fontSize,
          (line) => measureTextElementLine(styled, line),
        ),
      };
      // Expand when text is placed past the canvas edge (e.g. viewport chrome).
      const withText = { ...current, elements: [...current.elements, element] };
      commitDocument(expandDocumentToFitBounds(withText, elementBounds(element), 0));
      beginTextEditingFromPointerDown(element.id, true);
      return;
    }

    // Shape tools keep post-place grips live: drag a handle to bend/resize
    // without switching to Select; empty space still starts a new shape.
    if (isShapeDrawTool(tool)) {
      const selectedElement = selectedId
        ? current.elements.find((element) => element.id === selectedId) ?? null
        : null;
      if (selectedElement) {
        const annotationHit = hitTestSelectedAnnotation(
          selectedElement,
          point,
          interactionRadius,
        );
        if (annotationHit?.kind === "resize") {
          gestureRef.current = {
            kind: "resize",
            pointerId: event.pointerId,
            handle: annotationHit.handle,
            element: selectedElement,
            initialBounds: annotationHit.bounds,
            currentBounds: annotationHit.bounds,
            initialDocument: current,
          };
          setResizePreviewBounds(annotationHit.bounds);
          setCanvasCursor(resizeCursor(annotationHit.handle));
          capturePointerTarget(event.currentTarget, event.pointerId);
          return;
        }
        if (
          annotationHit?.kind === "arrow-handle"
          && selectedElement.kind === "shape"
        ) {
          gestureRef.current = {
            kind: "arrow-handle",
            pointerId: event.pointerId,
            handle: annotationHit.handle,
            element: selectedElement,
            initialDocument: current,
          };
          setCanvasCursor("grabbing");
          setCurveHoverTip(null);
          capturePointerTarget(event.currentTarget, event.pointerId);
          return;
        }

        // The active shape tool also manipulates the shape it just created.
        // Keeping body clicks on that shape out of the draw path prevents a
        // double-click intended for curve insertion from creating tiny shapes.
        if (
          selectedElement.kind === "shape"
          && selectedElement.shape === tool
          && hitTestSelectedShapeBody(selectedElement, point, interactionRadius)
        ) {
          gestureRef.current = {
            kind: "move",
            pointerId: event.pointerId,
            origin: point,
            element: selectedElement,
            wasSelected: true,
            didMove: false,
            initialDocument: current,
          };
          setCanvasCursor("move");
          setCurveHoverTip(null);
          capturePointerTarget(event.currentTarget, event.pointerId);
          return;
        }
      }
    }

    const elementId = editorId();
    setSelectedId(null);
    const element: ScreenshotElement = tool === "pen"
      ? {
        id: elementId,
        kind: "path",
        x: point.x,
        y: point.y,
        points: [point],
        style: { ...defaultStyle, fill: null },
        locked: false,
        visible: true,
        opacity: 100,
        blendMode: "source-over",
      }
      : {
        id: elementId,
        kind: "shape",
        shape: tool as ShapeKind,
        x: point.x,
        y: point.y,
        endX: point.x,
        endY: point.y,
        controls: [],
        style: {
          ...defaultStyle,
          fill: tool === "rectangle" || tool === "ellipse" ? defaultStyle.fill : null,
        },
        locked: false,
        visible: true,
        opacity: 100,
        blendMode: "source-over",
      };
    gestureRef.current = {
      kind: "draw",
      pointerId: event.pointerId,
      elementId,
      initialDocument: current,
    };
    replaceDocument({ ...current, elements: [...current.elements, element] });
    capturePointerTarget(event.currentTarget, event.pointerId);
  };

  const startPointer = (event: React.PointerEvent<HTMLCanvasElement>) => {
    startPointerAt(event, canvasPoint(event));
  };

  /**
   * Pointer on the checkerboard / empty viewport chrome around the canvas.
   * Drawing tools and crop can start off-canvas; Select clears the current selection.
   */
  const startOutsidePointer = (event: React.PointerEvent<HTMLDivElement>) => {
    // Only the viewport padding/empty area — not the canvas surface or overlays.
    if (event.target !== event.currentTarget) return;
    if (event.button !== 0) return;
    // Command/Ctrl / middle-button pan is handled in capture phase.
    if (modifierPanRef.current || event.metaKey || event.ctrlKey) return;
    if (panGestureRef.current) return;
    // Remove-bg needs image pixels; ignore starts in the chrome.
    if (tool === "remove-bg") return;
    startPointerAt(event, canvasPoint(event));
  };

  const movePointer = (event: React.PointerEvent<Element>) => {
    const gesture = gestureRef.current;
    const point = canvasPoint(event);
    if (!gesture || gesture.pointerId !== event.pointerId) {
      if (tool === "remove-bg") {
        syncRemoveBgHoverCursor(event, point);
        return;
      }
      if (tool === "select" || isShapeDrawTool(tool)) {
        const current = documentRef.current;
        const interactionRadius = 10 / Math.max(0.01, displayScale);
        const selectedElement = selectedId && current
          ? current.elements.find((element) => element.id === selectedId) ?? null
          : null;
        if (selectedElement) {
          const annotationHit = hitTestSelectedAnnotation(
            selectedElement,
            point,
            interactionRadius,
          );
          if (
            selectedElement.kind === "shape"
            && isCurveableStrokeShape(selectedElement)
          ) {
            const hint = curveStrokeHoverHint(
              selectedElement,
              point,
              interactionRadius,
            );
            setCurveHoverTip(
              hint
                ? { text: hint, clientX: event.clientX, clientY: event.clientY }
                : null,
            );
            if (annotationHit?.kind === "arrow-handle") {
              setCanvasCursor(
                annotationHit.handle.kind === "start"
                  || annotationHit.handle.kind === "end"
                  ? "move"
                  : "grab",
              );
              return;
            }
            if (hint) {
              setCanvasCursor("pointer");
              return;
            }
          } else {
            setCurveHoverTip(null);
          }
          if (annotationHit?.kind === "resize") {
            setCanvasCursor(resizeCursor(annotationHit.handle));
            return;
          }
          if (
            tool !== "select"
            && selectedElement.kind === "shape"
            && selectedElement.shape === tool
            && hitTestSelectedShapeBody(selectedElement, point, interactionRadius)
          ) {
            setCanvasCursor("move");
            return;
          }
        } else {
          setCurveHoverTip(null);
        }
        if (tool === "select") {
          const hovered = current
            ? hitTestElement(current.elements, point, interactionRadius)
            : null;
          // Unselected curveable strokes: light discovery tip when hovering the path.
          if (
            hovered
            && hovered.kind === "shape"
            && isCurveableStrokeShape(hovered)
            && hovered.id !== selectedId
            && !hovered.locked
          ) {
            const closest = closestPointOnArrow(hovered, point);
            const pathHitRadius = Math.max(
              interactionRadius,
              hovered.style.strokeWidth * 2 + interactionRadius * 0.6,
            );
            if (closest.distance <= pathHitRadius) {
              setCurveHoverTip({
                text: "Click to select · double-click path to add curve points",
                clientX: event.clientX,
                clientY: event.clientY,
              });
            }
          }
          setCanvasCursor(hovered ? "move" : undefined);
          return;
        }
        // Shape tools: default crosshair for a new draw when not on a grip.
        setCanvasCursor(undefined);
        return;
      }
      setCurveHoverTip(null);
      setCanvasCursor(undefined);
      return;
    }
    setCurveHoverTip(null);
    if (gesture.kind === "crop") {
      const bounds = documentRef.current ?? { width: 1, height: 1 };
      const next = cropDragAspectRatio({
        preset: cropAspect,
        shiftKey: event.shiftKey,
        origin: gesture.origin,
        current: point,
        bounds,
        shiftAspect: gesture.shiftAspect,
        liveRect: gesture.lastRect,
      });
      const rect = boundedCropRect(
        gesture.origin,
        point,
        bounds,
        next.aspectRatio,
      );
      gestureRef.current = {
        ...gesture,
        shiftAspect: next.shiftAspect,
        lastRect: rect,
      };
      setCropSelection(rect);
      return;
    }
    if (gesture.kind === "arrow-handle") {
      const handle = gesture.handle;
      let next = gesture.element;
      if (handle.kind === "start") {
        next = { ...gesture.element, x: point.x, y: point.y };
      } else if (handle.kind === "end") {
        next = { ...gesture.element, endX: point.x, endY: point.y };
      } else if (handle.kind === "starter-control") {
        const controls = arrowStarterControls(gesture.element);
        controls[handle.index] = { x: point.x, y: point.y };
        next = { ...gesture.element, controls };
      } else {
        const controlIndex = handle.index;
        const controls = gesture.element.controls.map((control, index) => (
          index === controlIndex ? { x: point.x, y: point.y } : control
        ));
        next = { ...gesture.element, controls };
      }
      if (handle.kind === "start" || handle.kind === "end") {
        next = scaleArrowStrokeForLength(gesture.element, next);
      }
      setCanvasCursor("grabbing");
      setCanvasExpandPreview(canvasExpandPreviewForBounds(
        elementBounds(next),
        gesture.initialDocument,
        next,
      ));
      replaceDocument(replaceElement(
        gesture.initialDocument,
        gesture.element.id,
        next,
      ));
      return;
    }
    if (gesture.kind === "move") {
      setCanvasCursor("move");
      const directTextEditCandidate = gesture.element.kind === "text" && gesture.wasSelected;
      const didMove = gesture.didMove || Math.hypot(
        point.x - gesture.origin.x,
        point.y - gesture.origin.y,
      ) > (directTextEditCandidate ? 2 / Math.max(0.01, displayScale) : 0);
      if (didMove !== gesture.didMove) {
        gestureRef.current = { ...gesture, didMove };
      }
      if (directTextEditCandidate && !didMove) return;
      const snapThreshold = ALIGNMENT_SNAP_SCREEN_PX / Math.max(0.01, displayScale);
      const free = translateElement(
        gesture.element,
        point.x - gesture.origin.x,
        point.y - gesture.origin.y,
      );
      const freeBounds = elementBounds(free);
      const lines = collectAlignmentSnapLines(
        gesture.initialDocument,
        gesture.element.id,
      );
      const snapped = snapTranslatedBounds(freeBounds, lines, snapThreshold);
      const deltaX = snapped.bounds.x - freeBounds.x;
      const deltaY = snapped.bounds.y - freeBounds.y;
      const moved = (deltaX !== 0 || deltaY !== 0)
        ? translateElement(free, deltaX, deltaY)
        : free;
      const nextDocument = replaceElement(
        gesture.initialDocument,
        gesture.element.id,
        moved,
      );
      setAlignmentGuides(snapped.guides);
      setCanvasExpandPreview(canvasExpandPreviewForBounds(
        elementBounds(moved),
        gesture.initialDocument,
        moved,
      ));
      replaceDocument(nextDocument);
      return;
    }
    if (gesture.kind === "remove-bg") {
      const current = documentRef.current;
      const element = current?.elements.find((item) => item.id === gesture.elementId);
      // Keep the size-matched ring under the pointer for the whole stroke.
      showBrushCursor(event.clientX, event.clientY, gesture.mode);
      if (!element || element.kind !== "image") return;
      const pixel = documentPointToImagePixel(element, point);
      if (!pixel) return;
      gestureRef.current = { ...gesture, pendingPixel: pixel };
      scheduleRemoveBgBrushPreview();
      return;
    }

    if (gesture.kind === "resize") {
      setCanvasCursor(resizeCursor(gesture.handle));
      const minSize = 8 / Math.max(0.01, displayScale);
      const snapThreshold = ALIGNMENT_SNAP_SCREEN_PX / Math.max(0.01, displayScale);
      // Hold Shift while dragging a corner to keep the original aspect ratio.
      // Text labels always scale as a unit so the plate cannot stretch
      // independently of the glyphs (including outline drags mapped to corners).
      const lockAspectRatio = event.shiftKey || gesture.element.kind === "text";
      const freeBounds = resizeBoundsFromHandle(
        gesture.initialBounds,
        gesture.handle,
        point,
        minSize,
        lockAspectRatio,
      );
      const lines = collectAlignmentSnapLines(
        gesture.initialDocument,
        gesture.element.id,
      );
      const snapped = snapResizedBounds(
        gesture.initialBounds,
        gesture.handle,
        freeBounds,
        lines,
        snapThreshold,
        minSize,
      );
      // Snap can nudge axes independently; re-apply the lock so Shift stays fixed-ratio.
      const nextBounds = lockAspectRatio
        ? resizeBoundsFromHandle(
          gesture.initialBounds,
          gesture.handle,
          resizeHandlePoint(snapped.bounds, gesture.handle),
          minSize,
          true,
        )
        : snapped.bounds;
      const resized = resizeElement(
        gesture.element,
        gesture.initialBounds,
        nextBounds,
      );
      gestureRef.current = { ...gesture, currentBounds: nextBounds };
      setResizePreviewBounds(
        resized.kind === "text" ? elementBounds(resized) : nextBounds,
      );
      setAlignmentGuides(snapped.guides);
      setCanvasExpandPreview(canvasExpandPreviewForBounds(
        nextBounds,
        gesture.initialDocument,
        resized,
      ));
      replaceDocument(replaceElement(
        gesture.initialDocument,
        gesture.element.id,
        resized,
      ));
      return;
    }
    // Freehand / shape draw: track the pointer (including past the canvas edge).
    const current = documentRef.current;
    if (!current || gesture.kind !== "draw") return;
    const element = current.elements.find(({ id }) => id === gesture.elementId);
    if (!element) return;
    let updated: ScreenshotElement | null = null;
    if (element.kind === "path") {
      const last = element.points.at(-1);
      if (last && Math.hypot(point.x - last.x, point.y - last.y) < 1.5 / displayScale) {
        updated = element;
      } else {
        updated = {
          ...element,
          points: [...element.points, point],
        };
      }
    } else if (element.kind === "shape") {
      updated = {
        ...element,
        endX: point.x,
        endY: point.y,
      };
    }
    if (!updated) return;
    if (updated !== element) {
      replaceDocument(replaceElement(current, element.id, updated));
    }
    setCanvasExpandPreview(canvasExpandPreviewForBounds(
      elementBounds(updated),
      gesture.initialDocument,
      updated,
    ));
  };

  const finishPointer = (event: React.PointerEvent<Element>) => {
    const gesture = gestureRef.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    setResizePreviewBounds(null);
    setAlignmentGuides([]);
    setCanvasExpandPreview(null);
    releasePointerTarget(event.currentTarget, event.pointerId);

    if (gesture.kind === "remove-bg") {
      const element = gesture.initialDocument.elements.find(
        (item) => item.id === gesture.elementId,
      );
      const releasePixel = event.type === "pointerup" && element?.kind === "image"
        ? documentPointToImagePixel(element, canvasPoint(event))
        : null;
      if (releasePixel) {
        gestureRef.current = { ...gesture, pendingPixel: releasePixel };
      }
      if (removeBgPreviewFrameRef.current !== null) {
        window.cancelAnimationFrame(removeBgPreviewFrameRef.current);
        removeBgPreviewFrameRef.current = null;
      }
      const finishedGesture = flushRemoveBgBrushPreview() ?? gesture;
      gestureRef.current = null;
      // Stay on the brush ring after a stroke ends (size still tracks the slider).
      syncRemoveBgHoverCursor(event, canvasPoint(event), {
        mode: finishedGesture.mode,
        forceOverImage: true,
      });
      if (!finishedGesture.changed) {
        removeBgLiveRef.current = null;
        paintEditorCanvas();
        return;
      }
      try {
        // Reuse the already-painted canvas as the decoded cache entry. Waiting
        // for a new data-URL Image to decode left the editor blank for a frame.
        const nextSrc = finishedGesture.workingCanvas.toDataURL("image/png");
        imageCacheRef.current.set(nextSrc, {
          image: finishedGesture.workingCanvas,
          status: "loaded",
        });
        const next = applyImageBackgroundEdit(
          finishedGesture.initialDocument,
          finishedGesture.elementId,
          nextSrc,
          finishedGesture.sourceBeforeEdit,
        );
        removeBgLiveRef.current = null;
        // Commit with undo — initialDocument is the pre-stroke snapshot.
        commitDocument(next);
        if (exportFormat === "jpeg") {
          setExportFormat("png");
        }
      } catch (reason) {
        removeBgLiveRef.current = null;
        paintEditorCanvas();
        setError(String(reason));
      }
      return;
    }

    gestureRef.current = null;
    if (gesture.kind === "crop") {
      setCanvasCursor(undefined);
      return;
    }

    setCanvasCursor(undefined);
    setBrushCursor(null);
    setWandLoupe(null);

    let current = documentRef.current;
    if (!current) return;
    const releasePoint = gesture.kind === "move" ? canvasPoint(event) : null;

    if (
      gesture.kind === "move"
      && gesture.element.kind === "text"
      && gesture.wasSelected
      && !gesture.didMove
      && releasePoint
      && Math.hypot(releasePoint.x - gesture.origin.x, releasePoint.y - gesture.origin.y)
        <= 2 / Math.max(0.01, displayScale)
    ) {
      beginTextEditing(gesture.element.id);
      return;
    }

    // Past-edge moves, resizes, arrow edits, and new drawings expand the canvas on release.
    if (
      gesture.kind === "resize"
      || gesture.kind === "move"
      || gesture.kind === "arrow-handle"
      || gesture.kind === "draw"
    ) {
      const elementId = gesture.kind === "draw" ? gesture.elementId : gesture.element.id;
      const element = current.elements.find(({ id }) => id === elementId);
      if (element) {
        const expanded = expandDocumentToFitBounds(current, elementBounds(element), 0);
        if (expanded !== current) {
          replaceDocument(expanded);
          current = expanded;
        }
      }
      setSaved(null);
      clearSuccess();
    }

    // Keep the just-drawn shape selected so curve/resize grips show immediately
    // without switching to Select & move.
    if (gesture.kind === "draw") {
      const drawn = current.elements.find(({ id }) => id === gesture.elementId);
      if (drawn && drawn.kind === "shape") {
        setSelectedId(drawn.id);
      }
    }

    if (JSON.stringify(current) === JSON.stringify(gesture.initialDocument)) return;
    setUndoStack((stack) => [...stack.slice(-99), gesture.initialDocument]);
    setRedoStack([]);
  };

  const handleCanvasDoubleClick = (event: React.MouseEvent<HTMLCanvasElement>) => {
    const current = documentRef.current;
    // Curve point add/remove works with Select or any shape tool while a stroke is selected.
    if (
      !current
      || (tool !== "select" && !isShapeDrawTool(tool))
      || event.button !== 0
    ) {
      return;
    }
    const point = clientToDocumentPoint(event.clientX, event.clientY);
    const interactionRadius = 10 / Math.max(0.01, displayScale);

    // Prefer the selected line/arrow so double-click on its path adds a control.
    const selectedElement = selectedId
      ? current.elements.find((element) => element.id === selectedId) ?? null
      : null;
    if (
      selectedElement
      && selectedElement.kind === "shape"
      && isCurveableStrokeShape(selectedElement)
      && !selectedElement.locked
    ) {
      const handle = hitTestArrowHandle(selectedElement, point, interactionRadius);
      if (handle?.kind === "control") {
        event.preventDefault();
        commitDocument(replaceElement(
          current,
          selectedElement.id,
          removeArrowControl(selectedElement, handle.index),
        ));
        return;
      }
      if (
        handle?.kind === "start"
        || handle?.kind === "end"
        || handle?.kind === "starter-control"
      ) {
        // Starter dots are drag affordances; endpoints are move affordances.
        return;
      }
      const closest = closestPointOnArrow(selectedElement, point);
      const pathHitRadius = Math.max(
        interactionRadius,
        selectedElement.style.strokeWidth * 2 + 6 / Math.max(0.01, displayScale),
      );
      if (closest.distance <= pathHitRadius) {
        const updated = insertArrowControl(selectedElement, closest.point);
        if (updated) {
          event.preventDefault();
          commitDocument(replaceElement(current, selectedElement.id, updated));
          return;
        }
      }
    }

    // Double-click an unselected line/arrow path: select it and add a curve point.
    const hit = hitTestElement(current.elements, point, interactionRadius);
    if (
      hit
      && hit.kind === "shape"
      && isCurveableStrokeShape(hit)
      && !hit.locked
      && hit.id !== selectedId
    ) {
      const closest = closestPointOnArrow(hit, point);
      const pathHitRadius = Math.max(
        interactionRadius,
        hit.style.strokeWidth * 2 + 6 / Math.max(0.01, displayScale),
      );
      if (closest.distance <= pathHitRadius) {
        event.preventDefault();
        setSelectedId(hit.id);
        const updated = insertArrowControl(hit, closest.point);
        if (updated) {
          commitDocument(replaceElement(current, hit.id, updated));
          return;
        }
        return;
      }
    }

    if (tool === "select" && hit?.kind === "text") {
      event.preventDefault();
      beginTextEditing(hit.id);
    }
  };

  const applyCrop = () => {
    const current = documentRef.current;
    if (!current || !cropSelection) return;
    commitDocument(cropDocument(current, cropSelection));
    setCropSelection(null);
    setSelectedId(null);
    setTool("select");
  };

  const applyTrimEdges = () => {
    const current = documentRef.current;
    if (!current) return;
    const trimmed = trimDocumentToContent(current);
    if (trimmed === current) return;
    commitDocument(trimmed);
    setCropSelection(null);
    setTrimEdgesHover(false);
  };

  const transformImageLayer = useCallback((elementId: string, action: ImageTransformAction) => {
    const current = documentRef.current;
    const element = current?.elements.find(({ id }) => id === elementId);
    if (!current || element?.kind !== "image") return;

    const rotates = action === "rotate-clockwise" || action === "rotate-counterclockwise";
    const visibleElements = current.elements.filter((candidate) => candidate.visible);
    const fillsCanvas = element.visible
      && visibleElements.length === 1
      && visibleElements[0].id === element.id
      && Math.abs(element.x) < 0.01
      && Math.abs(element.y) < 0.01
      && Math.abs(element.width - current.width) < 0.01
      && Math.abs(element.height - current.height) < 0.01;
    const transformed = transformImageElement(element, action);
    let next = replaceElement(current, element.id, transformed);

    // The common fresh-photo case should become portrait/landscape in one click.
    // In layered compositions, preserve the canvas and expand only if this layer
    // would otherwise be clipped; other annotations remain independent layers.
    if (rotates && fillsCanvas) {
      next = trimDocumentToContent(next);
    } else {
      next = expandDocumentToFitBounds(next, elementBounds(transformed), 0);
    }

    commitDocument(next);
    setSelectedId(element.id);
    setEditingTextId(null);
    setCropSelection(null);
    setTrimEdgesHover(false);
    setTool("select");
    setError("");
  }, [commitDocument]);

  /** Decode a cached layer image into natural-resolution pixels. */
  const readLayerImageData = (src: string): ImageData => {
    const cached = imageCacheRef.current.get(src);
    if (!cached || cached.status !== "loaded") {
      throw new Error("That image has not finished loading yet.");
    }
    return imageToImageData(cached.image);
  };

  const applyWandRemoveBackground = (
    element: EditorImageElement,
    point: EditorPoint,
    initialDocument: ScreenshotDocument,
  ) => {
    if (removeBgBusy) return;
    const pixel = documentPointToImagePixel(element, point);
    if (!pixel) {
      setError("Click inside an image layer to sample a color.");
      return;
    }
    setRemoveBgBusy(true);
    setError("");
    try {
      const working = readLayerImageData(element.src);
      const changed = removeColorToTransparent(
        working,
        pixel.x,
        pixel.y,
        wandTolerance,
        wandContiguous,
      );
      if (changed === 0) {
        setError("No matching pixels were found. Try a higher tolerance.");
        return;
      }
      const workingCanvas = imageDataToCanvas(working);
      const nextSrc = workingCanvas.toDataURL("image/png");
      imageCacheRef.current.set(nextSrc, {
        image: workingCanvas,
        status: "loaded",
      });
      const next = applyImageBackgroundEdit(
        initialDocument,
        element.id,
        nextSrc,
        element.src,
      );
      commitDocument(next);
      setSelectedId(element.id);
      // JPEG cannot keep transparency — nudge toward PNG when holes appear.
      if (exportFormat === "jpeg") {
        setExportFormat("png");
      }
    } catch (reason) {
      setError(String(reason));
    } finally {
      setRemoveBgBusy(false);
    }
  };

  const beginRemoveBgBrush = (
    event: React.PointerEvent<Element>,
    element: EditorImageElement,
    point: EditorPoint,
    mode: "erase" | "restore",
    initialDocument: ScreenshotDocument,
  ) => {
    if (removeBgBusy) return;
    const pixel = documentPointToImagePixel(element, point);
    if (!pixel) {
      setError("Paint on an image layer to edit its background.");
      return;
    }
    try {
      const workingData = readLayerImageData(element.src);
      let originalData: ImageData | null = null;
      if (mode === "restore") {
        if (!element.originalSrc) {
          setError("Nothing to restore yet — remove some background first.");
          return;
        }
        originalData = readLayerImageData(element.originalSrc);
      }
      const radius = brushRadiusInNaturalPixels(element, removeBgBrushSize);
      const stamped = strokeRemoveBackgroundBrush(
        workingData,
        pixel.x,
        pixel.y,
        pixel.x,
        pixel.y,
        radius,
        mode,
        originalData,
        DEFAULT_BRUSH_HARDNESS,
      );
      // Seed the live canvas once after the initial stamp. Pointer moves only
      // upload the small dirty rectangle touched by the next brush segment.
      const workingCanvas = imageDataToCanvas(workingData);
      removeBgLiveRef.current = { elementId: element.id, canvas: workingCanvas };
      gestureRef.current = {
        kind: "remove-bg",
        pointerId: event.pointerId,
        mode,
        elementId: element.id,
        sourceBeforeEdit: element.src,
        initialDocument,
        workingData,
        workingCanvas,
        originalData,
        radius,
        lastPixel: pixel,
        pendingPixel: null,
        changed: stamped > 0,
      };
      setSelectedId(element.id);
      paintEditorCanvas();
      setCanvasCursor("none");
      showBrushCursor(event.clientX, event.clientY, mode);
      capturePointerTarget(event.currentTarget, event.pointerId);
    } catch (reason) {
      removeBgLiveRef.current = null;
      paintEditorCanvas();
      setError(String(reason));
    }
  };

  const updateSelected = (updater: (element: ScreenshotElement) => ScreenshotElement) => {
    const current = documentRef.current;
    const element = current?.elements.find(({ id }) => id === selectedId);
    if (!current || !element) return;
    commitDocument(replaceElement(current, element.id, updater(element)));
  };

  const updateLayer = (
    elementId: string,
    updater: (element: ScreenshotElement) => ScreenshotElement,
  ) => {
    const current = documentRef.current;
    const element = current?.elements.find(({ id }) => id === elementId);
    if (!current || !element) return;
    commitDocument(replaceElement(current, element.id, updater(element)));
  };

  const beginLayerRename = (element: ScreenshotElement) => {
    if (element.kind !== "image") return;
    setLayerMenuId(null);
    setSelectedId(element.id);
    setTool("select");
    setCropSelection(null);
    setLayerRename({ id: element.id, value: element.name });
  };

  const finishLayerRename = () => {
    const rename = layerRename;
    setLayerRename(null);
    if (!rename) return;
    const name = rename.value.trim();
    if (!name) return;
    updateLayer(rename.id, (element) => (
      element.kind === "image" ? { ...element, name } : element
    ));
  };

  const moveLayer = (elementId: string, direction: "front" | "back") => {
    const current = documentRef.current;
    if (!current) return;
    const index = current.elements.findIndex(({ id }) => id === elementId);
    if (index < 0 || current.elements[index].locked) return;
    const target = direction === "front"
      ? current.elements.at(-1)
      : current.elements[0];
    if (!target || target.id === elementId) return;
    const elements = reorderScreenshotLayers(
      current.elements,
      elementId,
      target.id,
      direction === "front" ? "before" : "after",
    );
    if (elements === current.elements) return;
    commitDocument({ ...current, elements });
    setSelectedId(elementId);
    setLayerMenuId(null);
  };

  const dropLayer = (
    movedId: string,
    targetId: string,
    placement: LayerDropPlacement,
  ) => {
    const current = documentRef.current;
    if (!current) return;
    const elements = reorderScreenshotLayers(
      current.elements,
      movedId,
      targetId,
      placement,
    );
    if (elements === current.elements) return;
    commitDocument({ ...current, elements });
    setSelectedId(movedId);
    setTool("select");
  };

  const defaultImageDropGuide = (current: ScreenshotDocument): ImageDropGuide => {
    // Used only when a drop arrives before any drag-over pointer sample.
    const target = resolveImageDropTarget(current, selectedId);
    const point = {
      x: target.x + target.width / 2,
      y: target.y + target.height,
    };
    return {
      edge: "bottom",
      target,
      point,
      focus: stackDropLightFocusAtPoint(point, target),
    };
  };

  const setImageDropGuideState = (guide: ImageDropGuide | null) => {
    imageDropGuideRef.current = guide;
    setImageDropGuide(guide);
  };

  const loadDroppedFiles = async (files: File[], guide?: ImageDropGuide) => {
    // Tell the preview stack this drop stayed inside Captures so it does not
    // dismiss the source card when a native file drag ends over the editor.
    void invoke("mark_internal_file_drop").catch(() => undefined);
    const images = files.filter(isSupportedImageFile);
    if (images.length === 0) {
      setError("Drop PNG, JPEG, WebP, GIF, or another image file.");
      return;
    }
    const initial = documentRef.current;
    if (!initial) return;
    let next = initial;
    let lastId: string | null = null;
    let placement = guide ?? defaultImageDropGuide(initial);
    const createdUrls: string[] = [];
    try {
      for (const file of images) {
        // Prefer a blob object URL (cheap, revocable). Fall back to a data URL
        // if the webview rejects the blob load — historically our CSP omitted
        // blob: from img-src, which produced "could not be loaded" on drop.
        // Same-app drops from a preview card can also hand the webview an empty
        // File; loadImageFile then reads the PNG staged for the native drag.
        const image = await loadImageFile(file, {
          preparedBytes: () => invoke<number[]>("read_prepared_drag_image", {
            fileName: file.name,
          }),
        });
        const sourceArtifactId = await invoke<string | null>("prepared_drag_artifact_id", {
          fileName: file.name,
        }).catch(() => null);
        createdUrls.push(image.src);
        if (image.src.startsWith("blob:")) {
          objectUrlsRef.current.add(image.src);
        }
        imageCacheRef.current.set(image.src, { image, status: "loaded" });
        const position = positionImportedImageAtEdge(
          image.naturalWidth,
          image.naturalHeight,
          next,
          placement.target,
          placement.edge,
          placement.point,
        );
        const element: EditorImageElement = {
          id: editorId(),
          kind: "image",
          source: "imported",
          src: image.src,
          originalSrc: null,
          name: file.name,
          sourceArtifactId,
          x: position.x,
          y: position.y,
          width: position.width,
          height: position.height,
          naturalWidth: image.naturalWidth,
          naturalHeight: image.naturalHeight,
          locked: false,
          visible: true,
          opacity: 100,
          blendMode: "source-over",
        };
        next = expandDocumentForElement(
          next,
          element,
          imageDropExpandPadding(placement.edge),
        );
        lastId = element.id;
        const added = next.elements.find(({ id }) => id === element.id);
        if (added) {
          const target = elementBounds(added);
          const point = {
            x: target.x + target.width / 2,
            y: target.y + target.height,
          };
          placement = {
            edge: placement.edge,
            target,
            point,
            focus: stackDropLightFocusAtPoint(point, target),
          };
        }
      }
      commitDocument(next);
      setSelectedId(lastId);
      setTool("select");
      setImageRevision((revision) => revision + 1);
      setError("");
    } catch (reason) {
      createdUrls.forEach((url) => {
        if (url.startsWith("blob:")) {
          URL.revokeObjectURL(url);
          objectUrlsRef.current.delete(url);
        }
      });
      setError(String(reason));
    }
  };

  const updateCustomExportDimension = (dimension: "width" | "height", value: number) => {
    const current = documentRef.current;
    const next = Math.max(1, Math.min(MAX_SCREENSHOT_OUTPUT_DIMENSION, Math.round(value)));
    if (!current || !exportAspectLocked) {
      if (dimension === "width") setCustomExportWidth(next);
      else setCustomExportHeight(next);
      return;
    }
    if (dimension === "width") {
      setCustomExportWidth(next);
      setCustomExportHeight(Math.max(1, Math.min(
        MAX_SCREENSHOT_OUTPUT_DIMENSION,
        Math.round(next * current.height / current.width),
      )));
    } else {
      setCustomExportHeight(next);
      setCustomExportWidth(Math.max(1, Math.min(
        MAX_SCREENSHOT_OUTPUT_DIMENSION,
        Math.round(next * current.width / current.height),
      )));
    }
  };

  const renderFlattened = useCallback((): HTMLCanvasElement => {
    const current = documentRef.current;
    if (!current) throw new Error("The editor is still loading.");
    const missing = current.elements
      .filter((element): element is EditorImageElement => element.kind === "image" && element.visible)
      .find((element) => imageCacheRef.current.get(element.src)?.status !== "loaded");
    if (missing) throw new Error(`${missing.name} has not finished loading.`);
    const source = window.document.createElement("canvas");
    source.width = current.width;
    source.height = current.height;
    const sourceContext = source.getContext("2d");
    if (!sourceContext) throw new Error("Canvas rendering is unavailable.");
    renderScreenshot(sourceContext, current, imageCacheRef.current);
    const dimensions = screenshotOutputDimensions(
      current,
      exportSize,
      customExportWidth,
      customExportHeight,
    );
    if (dimensions.width * dimensions.height > MAX_SCREENSHOT_OUTPUT_PIXELS) {
      throw new Error("Output size is limited to 100 million pixels.");
    }
    if (dimensions.width === current.width && dimensions.height === current.height) return source;
    const output = window.document.createElement("canvas");
    output.width = dimensions.width;
    output.height = dimensions.height;
    const outputContext = output.getContext("2d");
    if (!outputContext) throw new Error("Canvas resizing is unavailable.");
    outputContext.imageSmoothingEnabled = true;
    outputContext.imageSmoothingQuality = "high";
    outputContext.drawImage(source, 0, 0, dimensions.width, dimensions.height);
    return output;
  }, [customExportHeight, customExportWidth, exportSize]);

  useEffect(() => {
    if (!editorDocument || !artifact) return;
    let cancelled = false;
    const timer = window.setTimeout(() => {
      if (cancelled) return;
      if (shouldUseOriginalFileSizeEstimate(
        artifact,
        editorDocument,
        baselineDocumentRef.current,
        exportFormat,
        exportSize,
        qualityMode,
      )) {
        setEstimatedBytes(artifact.size_bytes);
        setEstimatePending(false);
        return;
      }
      setEstimatePending(true);
      void (async () => {
        try {
          const canvas = renderFlattened();
          // PNG color quant and lossy WebP go through Rust so Est. size matches save.
          // JPEG stays in-browser (toBlob quality matches our encoder closely enough).
          let bytes: number;
          if (
            (exportFormat === "png" || exportFormat === "webp")
            && qualityMode !== "preserve"
          ) {
            const imagePng = await canvasPngBytes(canvas);
            const maxSizeBytes = qualityMode === "maximum"
              ? Number(maximumFileSize) * SCREENSHOT_FILE_SIZE_UNIT_BYTES[maximumFileSizeUnit]
              : null;
            bytes = await invoke<number>("estimate_screenshot_export", {
              imagePng,
              format: exportFormat,
              qualityMode,
              jpegQuality: Number(jpegQuality),
              maxSizeBytes: maxSizeBytes !== null && Number.isFinite(maxSizeBytes)
                ? Math.round(maxSizeBytes)
                : null,
              pngMaxColors: exportFormat === "png" && qualityMode === "compress"
                ? pngMaxColorsForQuality(jpegQuality)
                : null,
            });
          } else {
            const estimateQuality = exportFormat === "jpeg" && qualityMode !== "preserve"
              ? Number(jpegQuality)
              : 100;
            bytes = await estimateCanvasExportBytes(
              canvas,
              exportFormat,
              estimateQuality,
            );
          }
          if (!cancelled) {
            setEstimatedBytes(bytes);
            setEstimatePending(false);
          }
        } catch {
          if (!cancelled) {
            setEstimatedBytes(null);
            setEstimatePending(false);
          }
        }
      })();
    }, 220);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [
    artifact,
    customExportWidth,
    customExportHeight,
    editorDocument,
    exportFormat,
    exportSize,
    imageRevision,
    jpegQuality,
    maximumFileSize,
    maximumFileSizeUnit,
    qualityMode,
    renderFlattened,
  ]);

  const copyEditedImage = async () => {
    if (busy) return;
    setBusy("copying");
    setError("");
    clearSuccess();
    try {
      const imagePng = await canvasPngBytes(renderFlattened());
      await invoke("copy_screenshot_edit", { imagePng });
      showSuccess("copy", "Copied to clipboard");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(null);
    }
  };

  const saveEditedImage = async () => {
    if (!artifact || busy) return;
    const invalidFilename = screenshotFilenameError(filenameStem);
    if (invalidFilename) {
      setError(invalidFilename);
      return;
    }
    if (!destinationDirectory.trim()) {
      setError("Choose a destination folder for the edited screenshot.");
      return;
    }
    const maximumSizeText = qualityMode === "maximum"
      ? maximumFileSize.trim()
      : "";
    const maximumSizeBytes = maximumSizeText
      ? Math.floor(
        Number(maximumSizeText) * SCREENSHOT_FILE_SIZE_UNIT_BYTES[maximumFileSizeUnit],
      )
      : null;
    if (
      qualityMode === "maximum"
      && (!maximumSizeText
        || !Number.isFinite(maximumSizeBytes)
        || maximumSizeBytes === null
        || maximumSizeBytes < 10_000)
    ) {
      setError("Enter a maximum file size of at least 10 KB.");
      return;
    }
    // Keep the user-selected format. Compress/maximum only change how that format is encoded.
    // Maximum mode searches down from full quality; Compress uses the selected preset.
    const saveQuality = qualityMode === "compress" ? Number(jpegQuality) : 100;
    setBusy("saving");
    setError("");
    clearSuccess();
    try {
      const destinationPath = screenshotDestinationPath(
        destinationDirectory,
        filenameStem,
        exportFormat,
        artifact.path,
      );
      const overwriteSource = !savingCopy
        && !sourceMissing
        && artifact.path === destinationPath;
      const imagePng = await canvasPngBytes(renderFlattened());
      const result = await invoke<SavedScreenshotEdit>("save_screenshot_edit", {
        request: {
          artifact_id: artifact.id,
          destination_path: destinationPath,
          format: exportFormat,
          quality_mode: qualityMode,
          jpeg_quality: saveQuality,
          png_max_colors: exportFormat === "png" && qualityMode === "compress"
            ? pngMaxColorsForQuality(jpegQuality)
            : null,
          max_size_bytes: maximumSizeBytes,
          overwrite_source: overwriteSource,
          image_png: imagePng,
        },
      });
      // Always adopt the saved artifact so a first Captures-folder save becomes
      // the new original (Save overwrites it; Make a copy creates another file).
      setArtifact(result.artifact);
      if (result.artifact.path) {
        setMakeCopy(false);
        setFilenameStem(screenshotFileStem(result.artifact.path));
        setDestinationDirectory(screenshotParentDirectory(result.artifact.path));
      }
      if (documentRef.current) {
        baselineDocumentRef.current = documentRef.current;
      }
      setDraftRestored(false);
      draftSaveGenerationRef.current += 1;
      void invoke("discard_screenshot_editor_draft", { artifactId: result.artifact.id })
        .catch(() => undefined);
      // Window may still be keyed by the previous capture when Make a copy saved a new id.
      if (artifactId && artifactId !== result.artifact.id) {
        void invoke("discard_screenshot_editor_draft", { artifactId })
          .catch(() => undefined);
      }
      setSaved(result);
      showSuccess(
        "save",
        overwriteSource ? "Saved changes to the original" : `Saved ${result.path}`,
      );
      try {
        await invoke("reveal_artifact", { artifactId: result.artifact.id });
      } catch (reason) {
        setError(`The screenshot was saved, but its folder could not open: ${String(reason)}`);
      }
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(null);
    }
  };

  const chooseDestinationDirectory = async () => {
    if (!artifact || busy) return;
    setError("");
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Choose save location",
        defaultPath: destinationDirectory,
      });
      if (typeof selected === "string") {
        setDestinationDirectory(selected);
        if (artifact.path && selected !== screenshotParentDirectory(artifact.path)) {
          setMakeCopy(true);
        }
        clearSuccess();
      }
    } catch (reason) {
      setError(`Save location could not be changed: ${String(reason)}`);
    }
  };

  const showSavedFile = async () => {
    if (!saved) return;
    try {
      await invoke("reveal_artifact", { artifactId: saved.artifact.id });
    } catch (reason) {
      setError(String(reason));
    }
  };

  // Hooks must stay above the loading early-return.
  const canPreviewCompression = qualityMode === "compress" || qualityMode === "maximum";

  const revokeCompressPreviewUrls = useCallback(() => {
    const { before, after } = compressPreviewUrlsRef.current;
    if (before) URL.revokeObjectURL(before);
    if (after) URL.revokeObjectURL(after);
    compressPreviewUrlsRef.current = { before: null, after: null };
    setCompressPreviewAfterUrl(null);
  }, []);

  const loadCompressPreview = useCallback(async () => {
    if (!canPreviewCompression || !editorDocument || !artifact) return;
    const request = ++compressPreviewRequestRef.current;
    setCompressPreviewPending(true);
    setCompressPreviewError("");
    // Local until ownership transfers to compressPreviewUrlsRef; anything
    // still local by `finally` (stale response or error) gets revoked.
    let afterUrl: string | null = null;
    try {
      const canvas = renderFlattened();
      const beforePng = await canvasPngBytes(canvas);

      const maxSizeBytes = qualityMode === "maximum"
        ? Number(maximumFileSize) * SCREENSHOT_FILE_SIZE_UNIT_BYTES[maximumFileSizeUnit]
        : null;
      const preview = await invoke<{
        bytes: number[];
        sizeBytes: number;
        format: ExportFormat;
      }>("preview_screenshot_export", {
        imagePng: beforePng,
        format: exportFormat,
        qualityMode,
        jpegQuality: qualityMode === "compress" ? Number(jpegQuality) : 100,
        maxSizeBytes: maxSizeBytes !== null && Number.isFinite(maxSizeBytes)
          ? Math.round(maxSizeBytes)
          : null,
        pngMaxColors: exportFormat === "png" && qualityMode === "compress"
          ? pngMaxColorsForQuality(jpegQuality)
          : null,
      });
      const mime = exportFormat === "jpeg"
        ? "image/jpeg"
        : exportFormat === "webp"
          ? "image/webp"
          : "image/png";
      const afterBlob = new Blob([new Uint8Array(preview.bytes)], { type: mime });
      afterUrl = URL.createObjectURL(afterBlob);

      if (compressPreviewRequestRef.current !== request) return;
      revokeCompressPreviewUrls();
      compressPreviewUrlsRef.current = { before: null, after: afterUrl };
      setCompressPreviewAfterUrl(afterUrl);
      setCompressPreviewBeforeBytes(beforePng.length);
      setCompressPreviewAfterBytes(preview.sizeBytes);
      afterUrl = null;
    } catch (reason) {
      if (compressPreviewRequestRef.current === request) {
        setCompressPreviewError(String(reason));
        setCompressPreviewAfterUrl(null);
        setCompressPreviewAfterBytes(null);
      }
    } finally {
      if (afterUrl) URL.revokeObjectURL(afterUrl);
      if (compressPreviewRequestRef.current === request) {
        setCompressPreviewPending(false);
      }
    }
  }, [
    artifact,
    canPreviewCompression,
    editorDocument,
    exportFormat,
    jpegQuality,
    maximumFileSize,
    maximumFileSizeUnit,
    qualityMode,
    renderFlattened,
    revokeCompressPreviewUrls,
  ]);

  useEffect(() => {
    if (!canPreviewCompression) {
      revokeCompressPreviewUrls();
      setCompressPreviewPending(false);
      setCompressPreviewError("");
      setCompressPreviewBeforeBytes(null);
      setCompressPreviewAfterBytes(null);
      return;
    }
    const timer = window.setTimeout(() => {
      void loadCompressPreview();
    }, 280);
    return () => window.clearTimeout(timer);
  }, [
    canPreviewCompression,
    exportFormat,
    imageRevision,
    jpegQuality,
    loadCompressPreview,
    maximumFileSize,
    maximumFileSizeUnit,
    qualityMode,
    revokeCompressPreviewUrls,
  ]);

  useEffect(() => () => {
    revokeCompressPreviewUrls();
  }, [revokeCompressPreviewUrls]);

  if (!artifact || !editorDocument) {
    return (
      <main className="screenshot-editor screenshot-editor-loading">
        {error || "Loading screenshot…"}
      </main>
    );
  }

  const output = screenshotOutputDimensions(
    editorDocument,
    exportSize,
    customExportWidth,
    customExportHeight,
  );
  const formatRequiresCopy = sourceMissing
    || !screenshotPathMatchesFormat(artifact.path, exportFormat);
  const savingCopy = makeCopy || formatRequiresCopy;
  const sourceDirectory = artifact.path ? screenshotParentDirectory(artifact.path) : "";
  const sourceStem = artifact.path ? screenshotFileStem(artifact.path) : "";
  const maximumSizeBytes = qualityMode === "maximum"
    ? Number(maximumFileSize) * SCREENSHOT_FILE_SIZE_UNIT_BYTES[maximumFileSizeUnit]
    : null;
  const estimatedSizeLabel = estimatePending && estimatedBytes === null
    ? "Estimating…"
    : estimatedBytes === null
      ? "—"
      : maximumSizeBytes !== null
        && Number.isFinite(maximumSizeBytes)
        && maximumSizeBytes >= 10_000
        && estimatedBytes > maximumSizeBytes
        && (exportFormat === "jpeg" || exportFormat === "webp")
        ? `≤ ${formatFileSize(maximumSizeBytes)}`
        : `≈ ${formatFileSize(estimatedBytes)}`;
  // Size change versus the original file, mirroring the compare modal's −N%.
  const estimatedDeltaPercent = qualityMode !== "preserve"
    && estimatedBytes !== null
    && artifact.size_bytes > 0
    ? Math.round((estimatedBytes / artifact.size_bytes - 1) * 100)
    : null;
  const estimatedDeltaLabel = estimatedDeltaPercent === null || estimatedDeltaPercent === 0
    ? null
    : estimatedDeltaPercent < 0
      ? `−${Math.abs(estimatedDeltaPercent)}%`
      : `+${estimatedDeltaPercent}%`;
  const formatLabel = exportFormat === "jpeg"
    ? "JPEG"
    : exportFormat === "webp"
      ? "WebP"
      : "PNG";
  const showCompressQuality = qualityMode === "compress";

  const applyExportFormat = (format: ExportFormat) => {
    setExportFormat(format);
    setSaved(null);
    clearSuccess();
  };

  const applyQualityMode = (mode: ScreenshotQualityMode) => {
    setQualityMode(mode);
    setSaved(null);
    clearSuccess();
  };

  const updateMakeCopy = (enabled: boolean) => {
    if (formatRequiresCopy) return;
    setMakeCopy(enabled);
    if (enabled && filenameStem === sourceStem && destinationDirectory === sourceDirectory) {
      setFilenameStem(screenshotEditedFileStem(sourceStem));
    } else if (!enabled && (
      filenameStem === screenshotEditedFileStem(sourceStem)
      || filenameStem === `${sourceStem}-copy`
    )) {
      setFilenameStem(sourceStem);
      setDestinationDirectory(sourceDirectory);
    }
    setSaved(null);
    clearSuccess();
  };

  const updateImageDropGuide = (clientX: number, clientY: number) => {
    const canvas = canvasRef.current;
    const current = documentRef.current;
    if (!canvas || !current) return;
    const bounds = canvas.getBoundingClientRect();
    const point = {
      x: (clientX - bounds.left) * canvas.width / Math.max(1, bounds.width),
      y: (clientY - bounds.top) * canvas.height / Math.max(1, bounds.height),
    };
    setImageDropGuideState(imageDropGuideAtPoint(current, selectedId, point));
  };

  return (
    <main
      className={`screenshot-editor${dragActive ? " screenshot-editor-drag-active" : ""}`}
      onDragEnter={(event) => {
        if (!isFileTransfer(event.dataTransfer)) return;
        event.preventDefault();
        dropDepthRef.current += 1;
        setDragActive(true);
        if (!imageDropGuideRef.current) {
          setImageDropGuideState(defaultImageDropGuide(editorDocument));
        }
      }}
      onDragOver={(event) => {
        if (!isFileTransfer(event.dataTransfer)) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "copy";
        updateImageDropGuide(event.clientX, event.clientY);
      }}
      onDragLeave={(event) => {
        if (!dragActive) return;
        event.preventDefault();
        dropDepthRef.current = Math.max(0, dropDepthRef.current - 1);
        if (dropDepthRef.current === 0) {
          setDragActive(false);
          setImageDropGuideState(null);
          setDropToastAnchor(null);
        }
      }}
      onDrop={(event) => {
        if (!isFileTransfer(event.dataTransfer)) return;
        event.preventDefault();
        dropDepthRef.current = 0;
        setDragActive(false);
        setDropToastAnchor(null);
        // Prefer the latest pointer sample from dragover; React state can lag.
        const guide = imageDropGuideRef.current
          ?? imageDropGuide
          ?? defaultImageDropGuide(editorDocument);
        setImageDropGuideState(null);
        void loadDroppedFiles(Array.from(event.dataTransfer.files), guide);
      }}
    >
      <header className={`screenshot-editor-header${draftRestored ? " has-draft-banner" : ""}`}>
        {draftRestored && (
          <div className="screenshot-editor-draft-banner" role="status">
            <span>Restored unsaved edits from last time.</span>
            <div className="screenshot-editor-draft-banner-actions">
              <button type="button" onClick={discardRestoredDraft}>
                Discard
              </button>
              <button
                type="button"
                className="screenshot-editor-draft-banner-dismiss"
                onClick={() => setDraftRestored(false)}
                aria-label="Dismiss restored-edits notice"
              >
                Dismiss
              </button>
            </div>
          </div>
        )}
        {/* Window title already says "Captures screenshot editor"; keep chrome here only. */}
        <div className="screenshot-editor-header-main">
        <div className="screenshot-editor-title">
          <div className="screenshot-canvas-toolbar" role="group" aria-label="Canvas">
            <label className="screenshot-canvas-dim">
              <span>W</span>
              <NumberInput
                compact
                min={1}
                max={16_384}
                ariaLabel="Width"
                value={editorDocument.width}
                onChange={(width) => commitDocument(resizeDocumentCanvas(
                  editorDocument,
                  width,
                  editorDocument.height,
                ))}
              />
            </label>
            <span className="screenshot-canvas-dim-sep" aria-hidden="true">×</span>
            <label className="screenshot-canvas-dim">
              <span>H</span>
              <NumberInput
                compact
                min={1}
                max={16_384}
                ariaLabel="Height"
                value={editorDocument.height}
                onChange={(height) => commitDocument(resizeDocumentCanvas(
                  editorDocument,
                  editorDocument.width,
                  height,
                ))}
              />
            </label>
            <span className="screenshot-canvas-toolbar-split" aria-hidden="true" />
            <button
              type="button"
              className="screenshot-canvas-tool screenshot-canvas-trim"
              disabled={!canTrimEdges}
              title="Shrink the canvas to the edges of visible layers"
              onClick={applyTrimEdges}
              onPointerEnter={() => setTrimEdgesHover(true)}
              onPointerLeave={() => setTrimEdgesHover(false)}
              onFocus={() => setTrimEdgesHover(true)}
              onBlur={() => setTrimEdgesHover(false)}
            >
              <EditorIcon name="trim" />
              Trim edges
            </button>
            <CanvasBackgroundPicker
              value={editorDocument.background}
              onChange={(background) => commitDocument({ ...editorDocument, background })}
            />
          </div>
        </div>
        <div className="screenshot-editor-history-actions">
          <button type="button" disabled={undoStack.length === 0} onClick={undo} aria-label="Undo">
            <EditorIcon name="undo" />
          </button>
          <button type="button" disabled={redoStack.length === 0} onClick={redo} aria-label="Redo">
            <EditorIcon name="redo" />
          </button>
          <span className="screenshot-editor-zoom" role="group" aria-label="Canvas zoom controls">
            <button
              type="button"
              className={zoomMode === "fit" ? "active" : ""}
              aria-label="Fit canvas"
              title="Fit canvas to window"
              onClick={activateFitZoom}
            >
              <EditorIcon name="fit" />
            </button>
            <button
              type="button"
              aria-label="Zoom out"
              title="Zoom out"
              disabled={displayScale * 100 <= MIN_SCREENSHOT_ZOOM_PERCENT + 0.05}
              onClick={() => zoomBy(1 / KEYBOARD_ZOOM_FACTOR)}
            >
              <EditorIcon name="minus" />
            </button>
            <label className="screenshot-editor-zoom-slider">
              <input
                type="range"
                min={0}
                max={1}
                step="any"
                aria-label="Canvas zoom"
                aria-valuemin={MIN_SCREENSHOT_ZOOM_PERCENT}
                aria-valuemax={MAX_SCREENSHOT_ZOOM_PERCENT}
                aria-valuenow={clampScreenshotZoomPercent(
                  zoomMode === "fit" ? displayScale * 100 : zoom,
                )}
                aria-valuetext={
                  zoomMode === "fit"
                    ? `Fit (${screenshotZoomLabel(displayScale * 100)})`
                    : screenshotZoomLabel(zoom)
                }
                title="Drag to zoom · Pinch or Command/Ctrl + scroll also work"
                value={zoomPercentToSliderPosition(
                  zoomMode === "fit" ? displayScale * 100 : zoom,
                )}
                onChange={(event) => setManualZoom(
                  sliderPositionToZoomPercent(Number(event.target.value)),
                )}
              />
            </label>
            <button
              type="button"
              aria-label="Zoom in"
              title="Zoom in"
              disabled={displayScale * 100 >= MAX_SCREENSHOT_ZOOM_PERCENT - 0.05}
              onClick={() => zoomBy(KEYBOARD_ZOOM_FACTOR)}
            >
              <EditorIcon name="plus" />
            </button>
            <select
              className="screenshot-editor-zoom-presets"
              aria-label="Canvas zoom preset"
              title="Zoom presets"
              value={zoomMode === "fit" ? "fit" : String(zoom)}
              onChange={(event) => {
                if (event.target.value === "fit") activateFitZoom();
                else setManualZoom(Number(event.target.value));
              }}
            >
              <option value="fit">Fit</option>
              {zoomMode === "manual"
                && !SCREENSHOT_ZOOM_OPTIONS.some((option) => option === zoom)
                && <option value={String(zoom)}>{screenshotZoomLabel(zoom)}</option>}
              {SCREENSHOT_ZOOM_OPTIONS.map((option) => (
                <option key={option} value={String(option)}>{option}%</option>
              ))}
            </select>
          </span>
          <button type="button" className="screenshot-add-image" onClick={() => fileInputRef.current?.click()}>
            <EditorIcon name="image" /> Add images
          </button>
          <input
            ref={fileInputRef}
            type="file"
            accept="image/*"
            multiple
            hidden
            aria-label="Choose image layers"
            onChange={(event) => {
              void loadDroppedFiles(Array.from(event.target.files ?? []));
              event.target.value = "";
            }}
          />
        </div>
        </div>
      </header>

      <nav className="screenshot-tool-rail" aria-label="Screenshot tools">
        {TOOL_ITEMS.map((item) => (
          <button
            key={item.tool}
            type="button"
            className={tool === item.tool ? "active" : ""}
            aria-pressed={tool === item.tool}
            aria-label={`${item.label} (${item.shortcut})`}
            title={`${item.label} (${item.shortcut})`}
            onClick={() => {
              setEditingTextId(null);
              setTool(item.tool);
              if (item.tool !== "select") setSelectedId(null);
              if (item.tool !== "crop") setCropSelection(null);
            }}
          >
            <EditorIcon name={item.tool} />
            <span>{item.label}</span>
          </button>
        ))}
      </nav>

      <section
        ref={viewportRef}
        className={[
          "screenshot-canvas-viewport",
          panReady ? "is-pan-ready" : "",
          panActive ? "is-panning" : "",
        ].filter(Boolean).join(" ")}
        aria-label="Screenshot editing canvas"
        onPointerDownCapture={startPanPointer}
        onPointerMoveCapture={movePanPointer}
        onPointerUpCapture={finishPanPointer}
        onPointerCancelCapture={finishPanPointer}
        onPointerDown={startOutsidePointer}
        onPointerMove={movePointer}
        onPointerUp={finishPointer}
        onPointerCancel={finishPointer}
      >
        <button
          type="button"
          className={[
            "screenshot-canvas-recenter",
            canvasOffscreen ? "is-visible" : "",
          ].filter(Boolean).join(" ")}
          aria-hidden={!canvasOffscreen}
          tabIndex={canvasOffscreen ? 0 : -1}
          onClick={recenterCanvas}
        >
          Recenter
        </button>
        <div
          ref={surfaceRef}
          className={[
            "screenshot-canvas-surface",
            editorDocument.background ? "" : "transparent",
          ].filter(Boolean).join(" ")}
          style={{
            width: editorDocument.width * displayScale,
            height: editorDocument.height * displayScale,
            backgroundColor: editorDocument.background ?? undefined,
            transform: `translate(${viewPan.x}px, ${viewPan.y}px)`,
          }}
        >
          <canvas
            ref={canvasRef}
            width={editorDocument.width}
            height={editorDocument.height}
            style={{
              width: editorDocument.width * displayScale,
              height: editorDocument.height * displayScale,
              cursor: panActive ? "grabbing" : panReady ? "grab" : canvasCursor,
            }}
            className={`screenshot-canvas tool-${tool}`}
            onPointerDown={startPointer}
            onPointerMove={movePointer}
            onPointerUp={finishPointer}
            onPointerCancel={finishPointer}
            onPointerLeave={() => {
              setCurveHoverTip(null);
              setWandLoupe(null);
              // Leaving the canvas hides the brush ring; reappears on next hover.
              if (!gestureRef.current || gestureRef.current.kind !== "remove-bg") {
                setBrushCursor(null);
              }
            }}
            onDoubleClick={handleCanvasDoubleClick}
          />
          {canPreviewCompression && (
            <CompressionPreview
              className="is-embed"
              liveBefore
              beforeUrl={null}
              afterUrl={compressPreviewAfterUrl}
              beforeBytes={compressPreviewBeforeBytes}
              afterBytes={compressPreviewAfterBytes}
              pending={compressPreviewPending}
              error={compressPreviewError}
            />
          )}
          {editingText && inlineTextLayout && (
            <textarea
              ref={inlineTextRef}
              className={[
                "screenshot-inline-text-editor",
                isAutoWidthText(editingText) ? "is-auto-width" : "",
              ].filter(Boolean).join(" ")}
              aria-label="Edit text on canvas"
              autoFocus
              value={editingText.text}
              wrap={isAutoWidthText(editingText) ? "off" : "soft"}
              spellCheck
              style={{
                ...inlineTextLayout,
                color: editingText.outlined ? "transparent" : editingText.color,
                backgroundColor: editingText.background ?? "transparent",
                borderRadius: editingText.roundedBackground
                  ? textBackgroundRadius(
                    editingText,
                    (editingText.width
                      + (textHasBackgroundPlate(editingText)
                        ? textBackgroundPad(editingText.fontSize).x * 2
                        : 0)),
                    elementBounds(editingText).height,
                  ) * displayScale
                  : undefined,
                caretColor: editingText.color,
                fontFamily: fontFamily(editingText),
                fontSize: editingText.fontSize * displayScale,
                fontWeight: editingText.bold ? 700 : 400,
                fontStyle: editingText.italic ? "italic" : "normal",
                lineHeight: TEXT_LINE_HEIGHT_RATIO,
                textAlign: editingText.align,
                WebkitTextStroke: editingText.outlined
                  ? `${textOutlineWidth(editingText.fontSize) * displayScale}px ${editingText.color}`
                  : undefined,
                opacity: editingText.opacity / 100,
                mixBlendMode: editingText.blendMode === "source-over"
                  ? "normal"
                  : editingText.blendMode,
              }}
              onChange={(event) => {
                const nextText = event.target.value;
                updateLayer(editingText.id, (element) => (
                  element.kind === "text"
                    ? fitLiveText({ ...element, text: nextText })
                    : element
                ));
              }}
              onPointerDown={(event) => event.stopPropagation()}
              onBlur={(event) => {
                // The placing canvas click can steal focus after the textarea mounts.
                // Ignore only real pointer-driven blurs until that click finishes.
                if (suppressInlineTextBlurRef.current && event.nativeEvent.isTrusted) {
                  const input = inlineTextRef.current;
                  window.requestAnimationFrame(() => {
                    if (!input) return;
                    input.focus({ preventScroll: true });
                    if (selectInlineTextRef.current) input.select();
                  });
                  return;
                }
                setEditingTextId((current) => (
                  current === editingText.id ? null : current
                ));
              }}
              onKeyDown={(event) => {
                if (event.key !== "Escape") return;
                event.preventDefault();
                event.currentTarget.blur();
              }}
            />
          )}
          {dragActive && imageDropGuide && (
            <div
              className={`screenshot-drop-snap-guide edge-${imageDropGuide.edge}`}
              style={{
                left: imageDropGuide.target.x * displayScale,
                top: imageDropGuide.target.y * displayScale,
                width: imageDropGuide.target.width * displayScale,
                height: imageDropGuide.target.height * displayScale,
              }}
              aria-hidden="true"
            >
              {imageDropGuide.edge === "stack" ? (
                <StackDropLight guide={imageDropGuide} />
              ) : (
                <>
                  <div className="screenshot-drop-snap-bloom" />
                  <div className="screenshot-drop-snap-particles">
                    {DROP_SNAP_PARTICLES.map((particle) => (
                      <i
                        key={particle.id}
                        className="screenshot-drop-snap-particle"
                        style={{
                          // Stagger along the edge, travel distance, and timing for a
                          // continuous stream toward the drop side without JS loops.
                          ["--snap-along" as string]: particle.along,
                          ["--snap-travel" as string]: particle.travel,
                          ["--snap-delay" as string]: particle.delay,
                          ["--snap-duration" as string]: particle.duration,
                          ["--snap-size" as string]: particle.size,
                        }}
                      />
                    ))}
                  </div>
                </>
              )}
            </div>
          )}
          {alignmentGuides.map((guide) => (
            <div
              key={`${guide.orientation}-${guide.position}`}
              className={`screenshot-align-snap-guide ${guide.orientation}`}
              style={guide.orientation === "vertical"
                ? {
                  left: guide.position * displayScale,
                  top: 0,
                  height: editorDocument.height * displayScale,
                }
                : {
                  top: guide.position * displayScale,
                  left: 0,
                  width: editorDocument.width * displayScale,
                }}
              aria-hidden="true"
            />
          ))}
          {canvasExpandPreview && (
            <>
              <div
                className={[
                  "screenshot-canvas-expand-ghost",
                  ...canvasExpandPreview.edges.map((edge) => `edge-${edge}`),
                ].join(" ")}
                style={{
                  left: canvasExpandPreview.rect.x * displayScale,
                  top: canvasExpandPreview.rect.y * displayScale,
                  width: canvasExpandPreview.rect.width * displayScale,
                  height: canvasExpandPreview.rect.height * displayScale,
                  boxShadow: canvasExpandGhostBoxShadow(canvasExpandPreview.edges),
                }}
                aria-hidden="true"
              />
              <canvas
                ref={expandOverflowCanvasRef}
                className="screenshot-canvas-expand-overflow"
                width={Math.max(1, Math.ceil(canvasExpandPreview.rect.width))}
                height={Math.max(1, Math.ceil(canvasExpandPreview.rect.height))}
                style={{
                  left: canvasExpandPreview.rect.x * displayScale,
                  top: canvasExpandPreview.rect.y * displayScale,
                  width: canvasExpandPreview.rect.width * displayScale,
                  height: canvasExpandPreview.rect.height * displayScale,
                }}
                aria-hidden="true"
              />
              <div
                className={[
                  "screenshot-canvas-expand-hint",
                  ...canvasExpandPreview.edges.map((edge) => `edge-${edge}`),
                ].join(" ")}
                aria-hidden="true"
              >
                {canvasExpandPreview.edges.map((edge) => (
                  <div
                    key={edge}
                    className={`screenshot-canvas-expand-edge edge-${edge}`}
                  >
                    <div className="screenshot-canvas-expand-bloom" />
                    <div className="screenshot-canvas-expand-particles">
                      {DROP_SNAP_PARTICLES.map((particle) => (
                        <i
                          key={`${edge}-${particle.id}`}
                          className="screenshot-canvas-expand-particle"
                          style={{
                            ["--snap-along" as string]: particle.along,
                            ["--snap-travel" as string]: particle.travel,
                            ["--snap-delay" as string]: particle.delay,
                            ["--snap-duration" as string]: particle.duration,
                            ["--snap-size" as string]: particle.size,
                          }}
                        />
                      ))}
                    </div>
                  </div>
                ))}
                <span>Release to expand canvas</span>
              </div>
            </>
          )}
          {trimEdgesPreview && (
            <div className="screenshot-canvas-trim-hint" aria-hidden="true">
              {trimEdgesPreview.margins.top > 0 && (
                <div
                  className="screenshot-canvas-trim-region edge-top"
                  style={{
                    left: 0,
                    top: 0,
                    width: editorDocument.width * displayScale,
                    height: trimEdgesPreview.margins.top * displayScale,
                  }}
                />
              )}
              {trimEdgesPreview.margins.bottom > 0 && (
                <div
                  className="screenshot-canvas-trim-region edge-bottom"
                  style={{
                    left: 0,
                    top: (editorDocument.height - trimEdgesPreview.margins.bottom) * displayScale,
                    width: editorDocument.width * displayScale,
                    height: trimEdgesPreview.margins.bottom * displayScale,
                  }}
                />
              )}
              {trimEdgesPreview.margins.left > 0 && (
                <div
                  className="screenshot-canvas-trim-region edge-left"
                  style={{
                    left: 0,
                    top: trimEdgesPreview.margins.top * displayScale,
                    width: trimEdgesPreview.margins.left * displayScale,
                    height: (
                      editorDocument.height
                      - trimEdgesPreview.margins.top
                      - trimEdgesPreview.margins.bottom
                    ) * displayScale,
                  }}
                />
              )}
              {trimEdgesPreview.margins.right > 0 && (
                <div
                  className="screenshot-canvas-trim-region edge-right"
                  style={{
                    left: (editorDocument.width - trimEdgesPreview.margins.right) * displayScale,
                    top: trimEdgesPreview.margins.top * displayScale,
                    width: trimEdgesPreview.margins.right * displayScale,
                    height: (
                      editorDocument.height
                      - trimEdgesPreview.margins.top
                      - trimEdgesPreview.margins.bottom
                    ) * displayScale,
                  }}
                />
              )}
              <div
                className="screenshot-canvas-trim-keep"
                style={{
                  left: trimEdgesPreview.keepRect.x * displayScale,
                  top: trimEdgesPreview.keepRect.y * displayScale,
                  width: trimEdgesPreview.keepRect.width * displayScale,
                  height: trimEdgesPreview.keepRect.height * displayScale,
                }}
              />
              <div
                style={{
                  position: "absolute",
                  left: trimEdgesPreview.keepRect.x * displayScale,
                  top: trimEdgesPreview.keepRect.y * displayScale,
                  width: trimEdgesPreview.keepRect.width * displayScale,
                  height: trimEdgesPreview.keepRect.height * displayScale,
                }}
              >
                {trimEdgesPreview.edges.map((edge) => (
                  <div
                    key={edge}
                    className={`screenshot-canvas-trim-edge edge-${edge}`}
                  >
                    <div className="screenshot-canvas-trim-bloom" />
                    <div className="screenshot-canvas-trim-particles">
                      {DROP_SNAP_PARTICLES.map((particle) => (
                        <i
                          key={`trim-${edge}-${particle.id}`}
                          className="screenshot-canvas-trim-particle"
                          style={{
                            ["--snap-along" as string]: particle.along,
                            ["--snap-travel" as string]: particle.travel,
                            ["--snap-delay" as string]: particle.delay,
                            ["--snap-duration" as string]: particle.duration,
                            ["--snap-size" as string]: particle.size,
                          }}
                        />
                      ))}
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
        {dragActive && (
          <div
            className="screenshot-drop-overlay"
            style={dropToastAnchor ?? undefined}
            aria-hidden="true"
          >
            <EditorIcon name="image" />
            {/* Single-line toast; label length stays similar across edges so it does not reflow. */}
            <strong>{imageDropGuide ? imageDropLabel(imageDropGuide.edge) : "Drop image"}</strong>
          </div>
        )}
        {/* Outside the panned surface so position:fixed tracks the real pointer (client coords). */}
        {curveHoverTip && !panActive && !panReady && (
          <div
            className="screenshot-curve-hover-tip"
            role="tooltip"
            style={{
              left: curveHoverTip.clientX,
              top: curveHoverTip.clientY,
            }}
          >
            {curveHoverTip.text}
          </div>
        )}
        {/* Circular brush preview for erase/restore — diameter tracks brush size × zoom.
            Visibility is derived from tool/mode so we never need a setState-in-effect to clear it. */}
        {brushCursor
          && tool === "remove-bg"
          && removeBgMode !== "wand"
          && !panActive
          && !panReady && (
          <div
            ref={attachBrushCursor}
            className={[
              "screenshot-brush-cursor",
              brushCursor.mode === "restore" ? "is-restore" : "is-erase",
            ].join(" ")}
            aria-hidden="true"
            style={{
              width: removeBgBrushScreenDiameter(removeBgBrushSize, displayScale),
              height: removeBgBrushScreenDiameter(removeBgBrushSize, displayScale),
            }}
          />
        )}
        {/* Wand color loupe: zoomed natural pixels + hex so the sample color is obvious. */}
        {wandLoupe
          && tool === "remove-bg"
          && removeBgMode === "wand"
          && !panActive
          && !panReady && (
          <WandColorLoupe
            clientX={wandLoupe.clientX}
            clientY={wandLoupe.clientY}
            color={wandLoupe.color}
            canvasRef={wandLoupeCanvasRef}
          />
        )}
      </section>

      <aside className="screenshot-sidebar">
        <section className="screenshot-layers" aria-label="Layers">
          <div className="screenshot-layers-heading">
            <div>
              <strong>Layers</strong>
              <span>{editorDocument.elements.length}</span>
            </div>
            <button
              type="button"
              aria-label="Add image layer"
              title="Add image layer"
              onClick={() => fileInputRef.current?.click()}
            >
              <EditorIcon name="plus" />
            </button>
          </div>
          <ol className="screenshot-layer-list">
            {[...editorDocument.elements].reverse().map((element) => {
              const locked = element.locked;
              const previewOrientation = element.kind === "image"
                ? imageOrientationMatrix(element.orientation)
                : null;
              const dropPlacement = layerDropTarget?.id === element.id
                ? layerDropTarget.placement
                : null;
              return (
                <li
                  key={element.id}
                  className={[
                    selectedId === element.id ? "active" : "",
                    locked ? "locked" : "",
                    element.visible ? "" : "hidden",
                    draggedLayerId === element.id ? "dragging" : "",
                    dropPlacement ? `drop-${dropPlacement}` : "",
                  ].filter(Boolean).join(" ")}
                  draggable={!locked && layerRename?.id !== element.id}
                  onDragStart={(event) => {
                    if (locked) {
                      event.preventDefault();
                      return;
                    }
                    setLayerMenuId(null);
                    setDraggedLayerId(element.id);
                    event.dataTransfer.effectAllowed = "move";
                    event.dataTransfer.setData("application/x-captures-layer", element.id);
                  }}
                  onDragOver={(event) => {
                    const movedId = draggedLayerId
                      ?? event.dataTransfer.getData("application/x-captures-layer");
                    if (!movedId || movedId === element.id) return;
                    event.preventDefault();
                    event.stopPropagation();
                    event.dataTransfer.dropEffect = "move";
                    const bounds = event.currentTarget.getBoundingClientRect();
                    const placement = event.clientY < bounds.top + bounds.height / 2
                      ? "before"
                      : "after";
                    setLayerDropTarget({ id: element.id, placement });
                  }}
                  onDrop={(event) => {
                    const movedId = draggedLayerId
                      ?? event.dataTransfer.getData("application/x-captures-layer");
                    if (!movedId) return;
                    event.preventDefault();
                    event.stopPropagation();
                    dropLayer(
                      movedId,
                      element.id,
                      layerDropTarget?.id === element.id
                        ? layerDropTarget.placement
                        : "before",
                    );
                    setDraggedLayerId(null);
                    setLayerDropTarget(null);
                  }}
                  onDragEnd={() => {
                    setDraggedLayerId(null);
                    setLayerDropTarget(null);
                  }}
                >
                  <div
                    className="screenshot-layer-select"
                    role={layerRename?.id === element.id ? "group" : "button"}
                    tabIndex={layerRename?.id === element.id ? -1 : 0}
                    aria-pressed={layerRename?.id === element.id
                      ? undefined
                      : selectedId === element.id}
                    onClick={() => {
                      setEditingTextId(null);
                      setTool("select");
                      setCropSelection(null);
                      setSelectedId(element.id);
                    }}
                    onDoubleClick={(event) => {
                      if (element.kind !== "image") return;
                      event.preventDefault();
                      event.stopPropagation();
                      beginLayerRename(element);
                    }}
                    onKeyDown={(event) => {
                      if (event.key !== "Enter" && event.key !== " ") return;
                      event.preventDefault();
                      setEditingTextId(null);
                      setTool("select");
                      setCropSelection(null);
                      setSelectedId(element.id);
                    }}
                  >
                    <span
                      className="screenshot-layer-grip"
                      aria-hidden="true"
                      title={locked ? "Layer is locked" : "Drag to reorder"}
                    >
                      <EditorIcon name="grip" />
                    </span>
                    <span className="screenshot-layer-preview" aria-hidden="true">
                      {element.kind === "image"
                        ? (
                          <img
                            src={element.src}
                            alt=""
                            draggable={false}
                            style={{
                              transform: previewOrientation
                                ? `matrix(${previewOrientation.a}, ${previewOrientation.b}, ${previewOrientation.c}, ${previewOrientation.d}, 0, 0)`
                                : undefined,
                            }}
                          />
                        )
                        : <AnnotationLayerPreview element={element} />}
                    </span>
                    <span className="screenshot-layer-copy">
                      {layerRename?.id === element.id && element.kind === "image" ? (
                        <input
                          autoFocus
                          aria-label="Rename layer"
                          value={layerRename.value}
                          onFocus={(event) => event.currentTarget.select()}
                          onPointerDown={(event) => event.stopPropagation()}
                          onClick={(event) => event.stopPropagation()}
                          onDoubleClick={(event) => event.stopPropagation()}
                          onChange={(event) => setLayerRename({
                            id: element.id,
                            value: event.target.value,
                          })}
                          onBlur={finishLayerRename}
                          onKeyDown={(event) => {
                            event.stopPropagation();
                            if (event.key === "Enter") {
                              event.preventDefault();
                              finishLayerRename();
                            } else if (event.key === "Escape") {
                              event.preventDefault();
                              setLayerRename(null);
                            }
                          }}
                        />
                      ) : (
                        <strong title={element.kind === "image" ? "Double-click to rename" : undefined}>
                          {elementLayerName(element)}
                        </strong>
                      )}
                      <small>{elementKindLabel(element)}</small>
                    </span>
                  </div>
                  <span className="screenshot-layer-quick-actions">
                    <button
                      type="button"
                      className={element.visible ? "" : "active"}
                      aria-pressed={!element.visible}
                      aria-label={`${element.visible ? "Hide" : "Show"} ${elementLayerName(element)}`}
                      title={element.visible ? "Hide layer" : "Show layer"}
                      onClick={(event) => {
                        event.stopPropagation();
                        updateLayer(element.id, (current) => ({
                          ...current,
                          visible: !current.visible,
                        }));
                      }}
                    >
                      <EditorIcon name={element.visible ? "eye" : "eye-off"} />
                    </button>
                    <button
                      type="button"
                      className={locked ? "active" : ""}
                      aria-pressed={locked}
                      aria-label={`${locked ? "Unlock" : "Lock"} ${elementLayerName(element)}`}
                      title={locked ? "Unlock layer" : "Lock layer"}
                      onClick={(event) => {
                        event.stopPropagation();
                        setEditingTextId(null);
                        updateLayer(element.id, (current) => ({
                          ...current,
                          locked: !current.locked,
                        }));
                        setSelectedId(element.id);
                      }}
                    >
                      <EditorIcon name={locked ? "lock" : "unlock"} />
                    </button>
                    <span
                      className="screenshot-layer-menu"
                      ref={layerMenuId === element.id
                        ? (node) => { layerMenuRootRef.current = node; }
                        : undefined}
                    >
                      <button
                        type="button"
                        className={layerMenuId === element.id ? "active" : ""}
                        aria-label={`Layer settings for ${elementLayerName(element)}`}
                        aria-haspopup="dialog"
                        aria-expanded={layerMenuId === element.id}
                        title="Layer settings and actions"
                        ref={(node) => {
                          if (node) layerMenuTriggerRefs.current.set(element.id, node);
                          else layerMenuTriggerRefs.current.delete(element.id);
                        }}
                        onClick={(event) => {
                          event.stopPropagation();
                          setEditingTextId(null);
                          setTool("select");
                          setCropSelection(null);
                          setSelectedId(element.id);
                          setLayerMenuId((openId) => (
                            openId === element.id ? null : element.id
                          ));
                        }}
                      >
                        <EditorIcon name="more" />
                      </button>
                      {layerMenuId === element.id && layerMenuPlacement && createPortal(
                        <div
                          ref={layerMenuPanelRef}
                          className="screenshot-layer-menu-panel"
                          role="dialog"
                          aria-label={`Layer settings for ${elementLayerName(element)}`}
                          style={{
                            top: layerMenuPlacement.top,
                            bottom: layerMenuPlacement.bottom,
                            left: layerMenuPlacement.left,
                            maxHeight: layerMenuPlacement.maxHeight,
                          }}
                          onClick={(event) => event.stopPropagation()}
                        >
                          <section className="screenshot-layer-menu-section screenshot-layer-menu-section-static">
                            <h2 className="screenshot-layer-menu-section-title">Appearance</h2>
                            <label className="screenshot-layer-menu-field">
                              <span className="screenshot-layer-menu-field-label">Blend mode</span>
                              <CustomSelect
                                ariaLabel="Blend mode"
                                value={element.blendMode}
                                options={LAYER_BLEND_MODE_OPTIONS}
                                onChange={(blendMode) => updateLayer(element.id, (current) => ({
                                  ...current,
                                  blendMode: blendMode as LayerBlendMode,
                                }))}
                              />
                            </label>
                            <label className="screenshot-layer-menu-field">
                              <span className="screenshot-layer-menu-field-label">Opacity</span>
                              <RangeSlider
                                ariaLabel="Layer opacity"
                                min={0}
                                max={100}
                                value={element.opacity}
                                valueText={`${element.opacity}%`}
                                onChange={(opacity) => updateLayer(element.id, (current) => ({
                                  ...current,
                                  opacity,
                                }))}
                              />
                            </label>
                          </section>

                          <div className="screenshot-layer-menu-scroll">
                            {element.kind === "image" && (
                              <section className="screenshot-layer-menu-section">
                                <h2 className="screenshot-layer-menu-section-title">Transform</h2>
                                <div
                                  className="screenshot-layer-menu-transform-grid"
                                  role="group"
                                  aria-label={`Image transforms for ${elementLayerName(element)}`}
                                >
                                  <button
                                    type="button"
                                    className="screenshot-layer-menu-tile"
                                    aria-label="Rotate image counterclockwise"
                                    title="Rotate this image layer 90° counterclockwise"
                                    onClick={() => transformImageLayer(element.id, "rotate-counterclockwise")}
                                  >
                                    <EditorIcon name="rotate-counterclockwise" />
                                    <span>Rotate left</span>
                                  </button>
                                  <button
                                    type="button"
                                    className="screenshot-layer-menu-tile"
                                    aria-label="Rotate image clockwise"
                                    title="Rotate this image layer 90° clockwise"
                                    onClick={() => transformImageLayer(element.id, "rotate-clockwise")}
                                  >
                                    <EditorIcon name="rotate-clockwise" />
                                    <span>Rotate right</span>
                                  </button>
                                  <button
                                    type="button"
                                    className="screenshot-layer-menu-tile"
                                    aria-label="Flip image horizontally"
                                    title="Mirror this image layer from left to right"
                                    onClick={() => transformImageLayer(element.id, "flip-horizontal")}
                                  >
                                    <EditorIcon name="flip-horizontal" />
                                    <span>Flip horizontal</span>
                                  </button>
                                  <button
                                    type="button"
                                    className="screenshot-layer-menu-tile"
                                    aria-label="Flip image vertically"
                                    title="Mirror this image layer from top to bottom"
                                    onClick={() => transformImageLayer(element.id, "flip-vertical")}
                                  >
                                    <EditorIcon name="flip-vertical" />
                                    <span>Flip vertical</span>
                                  </button>
                                </div>
                              </section>
                            )}

                            <section className="screenshot-layer-menu-section">
                              <h2 className="screenshot-layer-menu-section-title">Arrange</h2>
                              <div className="screenshot-layer-menu-actions" role="group" aria-label="Layer arrange">
                                <button
                                  type="button"
                                  className="screenshot-layer-menu-action"
                                  disabled={locked || element.id === editorDocument.elements.at(-1)?.id}
                                  title="Move this layer above every other layer"
                                  onClick={() => moveLayer(element.id, "front")}
                                >
                                  <span className="screenshot-layer-menu-action-icon" aria-hidden="true">
                                    <EditorIcon name="bring-front" />
                                  </span>
                                  <span className="screenshot-layer-menu-action-label">Bring to front</span>
                                </button>
                                <button
                                  type="button"
                                  className="screenshot-layer-menu-action"
                                  disabled={locked || element.id === editorDocument.elements[0]?.id}
                                  title="Move this layer below every other layer"
                                  onClick={() => moveLayer(element.id, "back")}
                                >
                                  <span className="screenshot-layer-menu-action-icon" aria-hidden="true">
                                    <EditorIcon name="send-back" />
                                  </span>
                                  <span className="screenshot-layer-menu-action-label">Send to back</span>
                                </button>
                              </div>
                            </section>

                            <section className="screenshot-layer-menu-section">
                              <h2 className="screenshot-layer-menu-section-title">Combine</h2>
                              <div className="screenshot-layer-menu-actions" role="group" aria-label="Layer combine">
                                <button
                                  type="button"
                                  className="screenshot-layer-menu-action"
                                  disabled={!canMergeLayerDown(editorDocument.elements, element.id)}
                                  title="Rasterize this layer together with the unlocked layer directly under it"
                                  onClick={() => { mergeLayerDown(element.id); }}
                                >
                                  <span className="screenshot-layer-menu-action-icon" aria-hidden="true">
                                    <EditorIcon name="merge-down" />
                                  </span>
                                  <span className="screenshot-layer-menu-action-label">Merge down</span>
                                </button>
                                <button
                                  type="button"
                                  className="screenshot-layer-menu-action"
                                  disabled={!canMergeVisibleLayers(editorDocument.elements)}
                                  title="Rasterize every visible layer into one image; hidden layers stay"
                                  onClick={() => { mergeVisibleLayers(); }}
                                >
                                  <span className="screenshot-layer-menu-action-icon" aria-hidden="true">
                                    <EditorIcon name="merge-visible" />
                                  </span>
                                  <span className="screenshot-layer-menu-action-label">Merge visible</span>
                                </button>
                                <button
                                  type="button"
                                  className="screenshot-layer-menu-action"
                                  disabled={!canFlattenLayers(editorDocument.elements, editorDocument.background)}
                                  title="Bake the canvas background and visible layers into one locked background layer; discard hidden layers"
                                  onClick={() => { flattenImage(); }}
                                >
                                  <span className="screenshot-layer-menu-action-icon" aria-hidden="true">
                                    <EditorIcon name="flatten" />
                                  </span>
                                  <span className="screenshot-layer-menu-action-label">Flatten image</span>
                                </button>
                              </div>
                            </section>
                          </div>

                          <section className="screenshot-layer-menu-footer">
                            <button
                              type="button"
                              className="screenshot-layer-menu-action"
                              title="Duplicate this layer (Command/Ctrl+D)"
                              onClick={() => { duplicateLayer(element.id); }}
                            >
                              <span className="screenshot-layer-menu-action-icon" aria-hidden="true">
                                <EditorIcon name="duplicate" />
                              </span>
                              <span className="screenshot-layer-menu-action-label">Duplicate</span>
                            </button>
                            <button
                              type="button"
                              className="screenshot-layer-menu-action danger"
                              disabled={locked}
                              title="Delete this layer"
                              onClick={() => { deleteLayer(element.id); }}
                            >
                              <span className="screenshot-layer-menu-action-icon" aria-hidden="true">
                                <EditorIcon name="trash" />
                              </span>
                              <span className="screenshot-layer-menu-action-label">Delete</span>
                            </button>
                          </section>
                        </div>,
                        document.body,
                      )}
                    </span>
                  </span>
                </li>
              );
            })}
          </ol>
        </section>

        <section className="screenshot-properties" aria-label="Tool properties">
        {/* Select tool is already indicated on the left rail; only show a heading
            when an element is selected or another tool has properties to configure. */}
        {(selected || tool !== "select") && (
          <div className="screenshot-properties-heading">
            <strong>{selected ? elementLabel(selected) : toolLabel(tool)}</strong>
          </div>
        )}

        {tool === "crop" && (
          <section className="screenshot-property-section">
            <label>
              Aspect ratio
              <select value={cropAspect} onChange={(event) => setCropAspect(event.target.value)}>
                <option value="free">Free</option>
                <option value="1:1">1 : 1</option>
                <option value="4:3">4 : 3</option>
                <option value="3:2">3 : 2</option>
                <option value="16:9">16 : 9</option>
              </select>
            </label>
            {cropSelection ? (
              <>
                <div className="screenshot-number-pair">
                  <label>Width<input value={cropSelection.width} readOnly aria-label="Crop width" /></label>
                  <label>Height<input value={cropSelection.height} readOnly aria-label="Crop height" /></label>
                </div>
                <div className="screenshot-property-actions">
                  <button type="button" onClick={() => setCropSelection(null)}>Clear</button>
                  <button
                    type="button"
                    className="primary cta-pulse"
                    onClick={applyCrop}
                  >
                    Apply crop
                  </button>
                </div>
                <p>Hold Shift while dragging to keep this aspect ratio.</p>
              </>
            ) : (
              <p>
                Drag over the area you want to keep. Start from outside the
                canvas to crop to an edge. Hold Shift to lock the current
                aspect ratio.
              </p>
            )}
          </section>
        )}

        {tool === "remove-bg" && (
          <section className="screenshot-property-section">
            <p>
              Make pixels transparent on image layers — color match (wand),
              paint away (erase), or paint back (restore). Not automatic subject
              cutout. Export as PNG or WebP to keep transparency.
            </p>
            <div className="screenshot-format-buttons" role="group" aria-label="Eraser mode">
              {REMOVE_BG_MODE_ITEMS.map((item) => (
                <button
                  key={item.mode}
                  type="button"
                  className={removeBgMode === item.mode ? "active" : ""}
                  aria-pressed={removeBgMode === item.mode}
                  onClick={() => setRemoveBgMode(item.mode)}
                >
                  {item.label}
                </button>
              ))}
            </div>
            {removeBgMode === "wand" ? (
              <>
                <label>
                  Tolerance
                  <RangeSlider
                    ariaLabel="Color tolerance"
                    min={0}
                    max={120}
                    value={wandTolerance}
                    valueText={`${wandTolerance}`}
                    marks={[
                      { value: 0, label: "0" },
                      { value: 36, label: "36" },
                      { value: 80, label: "80" },
                      { value: 120, label: "120" },
                    ]}
                    onChange={setWandTolerance}
                  />
                </label>
                <label className="screenshot-check-row">
                  <input
                    type="checkbox"
                    checked={wandContiguous}
                    onChange={(event) => setWandContiguous(event.target.checked)}
                  />
                  Contiguous only
                </label>
                <p>
                  {wandContiguous
                    ? "Click a color region to clear connected matching pixels."
                    : "Click a color to clear every similar pixel in the layer."}
                </p>
              </>
            ) : (
              <>
                <label>
                  Brush size
                  <RangeSlider
                    ariaLabel="Brush size"
                    min={4}
                    max={120}
                    value={removeBgBrushSize}
                    valueText={`${removeBgBrushSize} px`}
                    marks={[
                      { value: 4, label: "4" },
                      { value: 28, label: "28" },
                      { value: 64, label: "64" },
                      { value: 120, label: "120" },
                    ]}
                    onChange={setRemoveBgBrushSize}
                  />
                </label>
                <p>
                  {removeBgMode === "erase"
                    ? "Paint to erase toward transparency."
                    : "Paint to restore pixels from before transparency edits."}
                </p>
              </>
            )}
            {removeBgBusy && <p>Working…</p>}
          </section>
        )}

        {selected?.kind === "text" && (
          <section className="screenshot-property-section">
            <TextStylePicker
              value={textStylePreset(selected)}
              onChange={(preset) => {
                setDefaultTextStyle(preset);
                updateSelected((element) => (
                  element.kind === "text"
                    ? fitLiveText(applyTextStylePreset(element, preset))
                    : element
                ));
              }}
            />
            <label>
              Text
              <textarea
                rows={4}
                value={selected.text}
                onChange={(event) => updateSelected((element) => (
                  element.kind === "text"
                    ? fitLiveText({ ...element, text: event.target.value })
                    : element
                ))}
              />
            </label>
            <div className="screenshot-number-pair">
              <label>
                Font
                <select
                  value={selected.fontFamily}
                  onChange={(event) => updateSelected((element) => (
                    element.kind === "text"
                      ? fitLiveText({
                        ...element,
                        fontFamily: event.target.value as typeof element.fontFamily,
                        roundedBackground: event.target.value === "rounded"
                          ? element.roundedBackground
                          : false,
                      })
                      : element
                  ))}
                >
                  <option value="sans">Sans serif</option>
                  <option value="serif">Serif</option>
                  <option value="mono">Monospace</option>
                  <option value="rounded">Rounded</option>
                </select>
              </label>
              <label>
                Size
                <NumberInput
                  min={8}
                  max={512}
                  value={selected.fontSize}
                  onChange={(fontSize) => updateSelected((element) => (
                    element.kind === "text"
                      ? fitLiveText({ ...element, fontSize: Math.max(8, fontSize) })
                      : element
                  ))}
                />
              </label>
            </div>
            <div className="screenshot-format-buttons">
              <button
                type="button"
                className={selected.bold ? "active" : ""}
                aria-label="Bold"
                onClick={() => updateSelected((element) => (
                  element.kind === "text"
                    ? fitLiveText({ ...element, bold: !element.bold })
                    : element
                ))}
              >B</button>
              <button
                type="button"
                className={selected.italic ? "active" : ""}
                aria-label="Italic"
                onClick={() => updateSelected((element) => (
                  element.kind === "text"
                    ? fitLiveText({ ...element, italic: !element.italic })
                    : element
                ))}
              ><em>I</em></button>
              {(["left", "center", "right"] as const).map((align) => (
                <button
                  key={align}
                  type="button"
                  className={selected.align === align ? "active" : ""}
                  aria-label={`Align ${align}`}
                  onClick={() => updateSelected((element) => (
                    element.kind === "text" ? { ...element, align } : element
                  ))}
                >
                  <EditorIcon name={`align-${align}`} />
                </button>
              ))}
            </div>
            <ColorField
              label="Text color"
              value={selected.color}
              onChange={(color) => updateSelected((element) => (
                element.kind === "text" ? { ...element, color } : element
              ))}
            />
            <label className="screenshot-check-row">
              <input
                type="checkbox"
                checked={selected.background !== null}
                onChange={(event) => updateSelected((element) => (
                  element.kind === "text"
                    ? {
                      ...element,
                      background: event.target.checked ? "#111318" : null,
                      outlined: event.target.checked ? false : element.outlined,
                      roundedBackground: event.target.checked
                        ? false
                        : element.roundedBackground,
                    }
                    : element
                ))}
              />
              Text background
            </label>
            {selected.background && (
              <ColorField
                label="Background color"
                value={selected.background}
                onChange={(background) => updateSelected((element) => (
                  element.kind === "text" ? { ...element, background } : element
                ))}
              />
            )}
          </section>
        )}

        {selected?.kind === "image" && (
          <section className="screenshot-property-section">
            <div className="screenshot-number-pair">
              <label>
                Width
                <NumberInput
                  min={1}
                  max={16_384}
                  value={Math.round(selected.width)}
                  disabled={selected.locked}
                  onChange={(width) => updateSelected((element) => {
                    if (element.kind !== "image") return element;
                    const size = imageSizeAtWidth(element, width);
                    return { ...element, ...size };
                  })}
                />
              </label>
              <label>
                Height
                <NumberInput value={Math.round(selected.height)} readOnly hideSteppers />
              </label>
              <label>
                X
                <NumberInput
                  value={Math.round(selected.x)}
                  disabled={selected.locked}
                  onChange={(x) => updateSelected((element) => (
                    element.kind === "image" ? { ...element, x } : element
                  ))}
                />
              </label>
              <label>
                Y
                <NumberInput
                  value={Math.round(selected.y)}
                  disabled={selected.locked}
                  onChange={(y) => updateSelected((element) => (
                    element.kind === "image" ? { ...element, y } : element
                  ))}
                />
              </label>
            </div>
          </section>
        )}

        {(selected?.kind === "shape" || selected?.kind === "path") && (
          <section className="screenshot-property-section">
            <ColorField
              label="Stroke color"
              value={selected.style.color}
              onChange={(color) => updateSelected((element) => (
                element.kind === "shape" || element.kind === "path"
                  ? { ...element, style: { ...element.style, color } }
                  : element
              ))}
            />
            <label>
              Stroke width
              <RangeSlider
                ariaLabel="Stroke width"
                min={2}
                max={40}
                value={selected.style.strokeWidth}
                valueText={`${selected.style.strokeWidth} px`}
                onChange={(strokeWidth) => updateSelected((element) => (
                  element.kind === "shape" || element.kind === "path"
                    ? {
                      ...element,
                      style: { ...element.style, strokeWidth },
                    }
                    : element
                ))}
              />
            </label>
            {selected.kind === "shape" && (selected.shape === "rectangle" || selected.shape === "ellipse") && (
              <>
                <label className="screenshot-check-row">
                  <input
                    type="checkbox"
                    checked={selected.style.fill !== null}
                    onChange={(event) => updateSelected((element) => (
                      element.kind === "shape"
                        ? {
                          ...element,
                          style: {
                            ...element.style,
                            fill: event.target.checked ? `${element.style.color}55` : null,
                          },
                        }
                        : element
                    ))}
                  />
                  Filled shape
                </label>
                {selected.style.fill && (
                  <ColorField
                    label="Fill color"
                    value={selected.style.fill.slice(0, 7)}
                    onChange={(fill) => updateSelected((element) => (
                      element.kind === "shape"
                        ? { ...element, style: { ...element.style, fill: `${fill}88` } }
                        : element
                    ))}
                  />
                )}
              </>
            )}
            {selected.kind === "shape" && isCurveableStrokeShape(selected) && (
              <>
                {selected.controls.length <= 1 ? (
                  <label>
                    Curve
                    <RangeSlider
                      ariaLabel="Curve"
                      min={-100}
                      max={100}
                      value={Math.round(arrowBendAmount(selected) * 100)}
                      valueText={`${Math.round(arrowBendAmount(selected) * 100)}%`}
                      marks={[
                        { value: -100, label: "Left" },
                        { value: 0, label: "Straight" },
                        { value: 100, label: "Right" },
                      ]}
                      onChange={(bend) => updateSelected((element) => (
                        element.kind === "shape"
                          ? arrowWithBend(element, bend / 100)
                          : element
                      ))}
                    />
                  </label>
                ) : (
                  <div className="screenshot-property-actions">
                    <button
                      type="button"
                      onClick={() => updateSelected((element) => (
                        element.kind === "shape"
                          ? { ...element, controls: [] }
                          : element
                      ))}
                    >
                      {selected.shape === "arrow" ? "Straighten arrow" : "Straighten line"}
                    </button>
                  </div>
                )}
                <p>
                  Drag the curve dots to reshape. Double-click the path to add more
                  points; double-click a point to remove it.
                </p>
              </>
            )}
          </section>
        )}

        {!selected && tool !== "crop" && tool !== "select" && (
          <section className="screenshot-property-section">
            {tool === "text" ? (
              <>
                <TextStylePicker
                  label="New text style"
                  value={defaultTextStyle}
                  onChange={setDefaultTextStyle}
                />
                <label>
                  New text size
                  <NumberInput
                    min={8}
                    max={512}
                    value={defaultFontSize}
                    onChange={setDefaultFontSize}
                  />
                </label>
              </>
            ) : (
              <>
                <ColorField
                  label="New annotation color"
                  value={defaultStyle.color}
                  onChange={(color) => setDefaultStyle((style) => ({ ...style, color }))}
                />
                <label>
                  Stroke width
                  <RangeSlider
                    ariaLabel="Stroke width"
                    min={2}
                    max={40}
                    value={defaultStyle.strokeWidth}
                    valueText={`${defaultStyle.strokeWidth} px`}
                    onChange={(strokeWidth) => setDefaultStyle((style) => ({
                      ...style,
                      strokeWidth,
                    }))}
                  />
                </label>
              </>
            )}
          </section>
        )}

        </section>
      </aside>

      <footer className="screenshot-export-bar">
        <div className={`screenshot-export-options${exportSettingsOpen ? " is-open" : ""}`}>
          <div id="screenshot-export-settings" className="screenshot-export-settings">
          <div className="screenshot-export-control screenshot-export-format">
            <span>Format</span>
            <CustomSelect
              value={exportFormat}
              ariaLabel="Format"
              options={[
                { value: "png", label: "PNG" },
                { value: "jpeg", label: "JPEG" },
                { value: "webp", label: "WebP" },
              ]}
              onChange={(value) => applyExportFormat(value as ExportFormat)}
            />
          </div>
          <div className="screenshot-export-control screenshot-export-size">
            <span>Output size</span>
            <span className="screenshot-export-size-control">
              <CustomSelect
                value={exportSize}
                ariaLabel="Output size"
                options={[
                  { value: "original", label: "Original" },
                  { value: "75", label: "75%" },
                  { value: "50", label: "50%" },
                  { value: "custom", label: "Custom" },
                ]}
                onChange={(value) => {
                  const next = value as ExportSize;
                  if (next === "custom" && exportSize !== "custom") {
                    setCustomExportWidth(editorDocument.width);
                    setCustomExportHeight(editorDocument.height);
                  }
                  setExportSize(next);
                }}
              />
              <span className="screenshot-output-dimensions" aria-live="polite">
                {output.width} × {output.height}
              </span>
            </span>
          </div>
          {exportSize === "custom" && (
            <div className="screenshot-export-control screenshot-custom-dimensions">
              <span>Width × height</span>
              <div>
                <NumberInput
                  min={1}
                  max={MAX_SCREENSHOT_OUTPUT_DIMENSION}
                  value={customExportWidth}
                  ariaLabel="Custom output width"
                  onChange={(width) => updateCustomExportDimension("width", width)}
                />
                <span aria-hidden="true">×</span>
                <NumberInput
                  min={1}
                  max={MAX_SCREENSHOT_OUTPUT_DIMENSION}
                  value={customExportHeight}
                  ariaLabel="Custom output height"
                  onChange={(height) => updateCustomExportDimension("height", height)}
                />
                <button
                  type="button"
                  className={exportAspectLocked ? "active" : ""}
                  aria-label="Lock output aspect ratio"
                  aria-pressed={exportAspectLocked}
                  title="Lock output aspect ratio"
                  onClick={() => setExportAspectLocked((locked) => !locked)}
                >
                  <EditorIcon name={exportAspectLocked ? "lock" : "unlock"} />
                </button>
              </div>
            </div>
          )}
          <div className="screenshot-export-control screenshot-quality-mode">
            <span>Save quality</span>
            <CustomSelect
              value={qualityMode}
              ariaLabel="Save quality"
              options={[
                {
                  value: "preserve",
                  label: "Preserve quality",
                  description: "Original quality with no extra compression unless an edit requires it.",
                },
                {
                  value: "compress",
                  label: "Compress",
                  description: exportFormat === "png"
                    ? "Smaller PNG with Tiny through High quality presets."
                    : exportFormat === "webp"
                      ? "Smaller lossy WebP with Tiny through High quality presets."
                      : "Smaller JPEG with Tiny through High quality presets.",
                },
                {
                  value: "maximum",
                  label: "Maximum file size",
                  description: "Set a hard size limit for the saved file.",
                },
              ]}
              onChange={(value) => applyQualityMode(value as ScreenshotQualityMode)}
            />
          </div>
          {showCompressQuality && (
            <div className="screenshot-export-control screenshot-quality">
              <span>Quality</span>
              <CustomSelect
                value={jpegQuality}
                ariaLabel="Compression quality"
                options={SCREENSHOT_QUALITY_OPTIONS.map((option) => ({
                  value: option.value,
                  label: option.label,
                  description: screenshotQualityDescription(exportFormat, option),
                }))}
                onChange={(value) => setJpegQuality(value as ScreenshotQuality)}
              />
            </div>
          )}
          {qualityMode === "maximum" && (
            <div
              className="screenshot-export-control screenshot-maximum-size"
              title={exportFormat === "jpeg" || exportFormat === "webp"
                ? `${formatLabel} quality is lowered only when needed to meet this limit.`
                : `Uses stronger ${formatLabel} compression. If still over the limit, reduce dimensions or switch format.`}
            >
              <span>Maximum file size</span>
              <span className="screenshot-maximum-size-control">
                <NumberInput
                  min={maximumFileSizeUnit === "kb" ? 10 : maximumFileSizeUnit === "mb" ? 0.01 : 0.00001}
                  step={maximumFileSizeUnit === "kb" ? 1 : maximumFileSizeUnit === "mb" ? 0.01 : 0.00001}
                  value={maximumFileSize}
                  ariaLabel="Maximum file size"
                  onTextChange={setMaximumFileSize}
                />
                <CustomSelect
                  value={maximumFileSizeUnit}
                  ariaLabel="Screenshot file size unit"
                  options={[
                    { value: "kb", label: "KB" },
                    { value: "mb", label: "MB" },
                    { value: "gb", label: "GB" },
                  ]}
                  onChange={(value) => {
                    const nextUnit = value as ScreenshotFileSizeUnit;
                    const bytes = Number(maximumFileSize)
                      * SCREENSHOT_FILE_SIZE_UNIT_BYTES[maximumFileSizeUnit];
                    setMaximumFileSizeUnit(nextUnit);
                    if (Number.isFinite(bytes)) {
                      setMaximumFileSize(formatScreenshotMaximumFileSizeInput(bytes, nextUnit));
                    }
                  }}
                />
              </span>
            </div>
          )}
          <div className="screenshot-export-control screenshot-output-estimate-control" aria-live="polite">
            <span>Est. size</span>
            <strong
              className="screenshot-output-estimate"
              data-pending={estimatePending ? "true" : undefined}
              title="Estimated export file size for the current format, quality, and output size"
            >
              {estimatedSizeLabel}
              {estimatedDeltaLabel && !estimatePending && (
                <span
                  className={`screenshot-output-estimate-delta${estimatedDeltaPercent !== null && estimatedDeltaPercent < 0 ? " is-smaller" : " is-larger"}`}
                  title="Change from the original file size"
                >
                  {estimatedDeltaLabel}
                </span>
              )}
            </strong>
          </div>
          </div>
        </div>
        <div className="screenshot-save-row">
          <button
            type="button"
            className="screenshot-export-disclosure"
            aria-controls="screenshot-export-settings"
            aria-expanded={exportSettingsOpen}
            onClick={() => setExportSettingsOpen((open) => !open)}
          >
            <span className="screenshot-export-disclosure-label">Export settings</span>
            <span className="screenshot-export-summary">
              {formatLabel} · {output.width} × {output.height} · {estimatedSizeLabel}
            </span>
            <EditorIcon name="chevron-down" />
          </button>
          <div className="recording-filename screenshot-filename">
            <div className="recording-filename-heading">
              <label htmlFor="screenshot-save-filename">Filename</label>
              <div className="recording-destination">
                <span>Saving to</span>
                <output aria-label="Save location" title={destinationDirectory}>
                  {destinationDirectory}
                </output>
                <button
                  type="button"
                  aria-label="Change save location"
                  disabled={busy !== null}
                  onClick={() => void chooseDestinationDirectory()}
                >Change…</button>
              </div>
            </div>
            <span className="recording-filename-input">
              <input
                id="screenshot-save-filename"
                value={filenameStem}
                aria-label="Saved filename"
                spellCheck={false}
                disabled={busy !== null}
                onFocus={(event) => event.currentTarget.select()}
                onChange={(event) => {
                  const next = event.target.value;
                  setFilenameStem(next);
                  if (artifact.path && (next !== sourceStem || destinationDirectory !== sourceDirectory)) {
                    setMakeCopy(true);
                  }
                  setSaved(null);
                  setError("");
                  clearSuccess();
                }}
              />
              <strong>.{screenshotFormatExtension(exportFormat, artifact.path)}</strong>
            </span>
          </div>
          <div
            className={[
              "screenshot-export-status",
              error ? "has-error" : "",
              !error && success ? "has-success" : "",
            ].filter(Boolean).join(" ")}
          >
            <div
              className={[
                "screenshot-export-notice",
                error ? "error" : success ? "success" : "idle",
              ].join(" ")}
              role={error ? "alert" : success ? "status" : undefined}
              aria-live={error ? "assertive" : "polite"}
            >
              {error || success?.message || "\u00a0"}
            </div>
            {!error && (
              <div className="screenshot-export-hint">
                {sourceMissing
                  ? "The original was deleted. You can still copy or save this edit."
                  : qualityMode === "preserve"
                    ? savingCopy
                      ? `Keeps original quality as ${formatLabel} and saves a new file.`
                      : `Keeps original quality as ${formatLabel} and replaces the original.`
                    : qualityMode === "maximum"
                      ? exportFormat === "jpeg"
                        ? savingCopy
                          ? "The JPEG stays within the selected limit and saves as a new file."
                          : "The JPEG stays within the selected limit and replaces the original."
                        : savingCopy
                          ? `Compresses ${formatLabel} to aim for the size limit and saves a new file.`
                          : `Compresses ${formatLabel} to aim for the size limit and replaces the original.`
                      : qualityMode === "compress"
                        ? savingCopy
                          ? `Compressed ${formatLabel} saves as a new file and leaves the original untouched.`
                          : `Compressed ${formatLabel} replaces the original; turn on Make a copy to keep it.`
                        : savingCopy
                          ? "Save creates a new file and leaves the original untouched."
                          : "Save replaces the original; turn on Make a copy to keep it."}
              </div>
            )}
          </div>
          <div className="screenshot-export-actions">
            {!formatRequiresCopy && (
              <label
                className="recording-toggle screenshot-make-copy"
                title="Save as a new file and leave the original untouched"
              >
                <input
                  aria-label="Make a copy"
                  type="checkbox"
                  checked={makeCopy}
                  disabled={busy !== null}
                  onChange={(event) => updateMakeCopy(event.target.checked)}
                />
                <span className="recording-switch" aria-hidden="true" />
                <span>Make a copy</span>
              </label>
            )}
            {saved && <button type="button" onClick={() => void showSavedFile()}>Show in Folder</button>}
            <button
              type="button"
              className={success?.kind === "copy" ? "success" : undefined}
              disabled={busy !== null}
              onClick={() => void copyEditedImage()}
            >
              <EditorIcon name={success?.kind === "copy" ? "check" : "copy"} />
              {busy === "copying"
                ? "Copying…"
                : success?.kind === "copy"
                  ? "Copied"
                  : "Copy image"}
            </button>
            <button
              type="button"
              className="primary"
              disabled={busy !== null}
              onClick={() => void saveEditedImage()}
            >
              <EditorIcon name="save" />{busy === "saving" ? "Saving…" : "Save"}
            </button>
          </div>
        </div>
      </footer>
    </main>
  );
}

function TextStylePicker({
  label = "Text style",
  value,
  onChange,
}: {
  label?: string;
  value: TextStylePreset;
  onChange: (value: TextStylePreset) => void;
}) {
  const [open, setOpen] = useState(false);
  const [menuPosition, setMenuPosition] = useState({ top: 0, left: 0, width: 0 });
  const pickerRef = useRef<HTMLFieldSetElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const selectedStyle = TEXT_STYLE_ITEMS.find((item) => item.preset === value)
    ?? TEXT_STYLE_ITEMS[0];

  const positionMenu = useCallback(() => {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const triggerBounds = trigger.getBoundingClientRect();
    const menuHeight = menuRef.current?.getBoundingClientRect().height ?? 296;
    const gap = 5;
    const viewportPadding = 8;
    const roomBelow = window.innerHeight - triggerBounds.bottom - viewportPadding;
    const roomAbove = triggerBounds.top - viewportPadding;
    const openAbove = roomBelow < menuHeight && roomAbove > roomBelow;
    const requestedTop = openAbove
      ? triggerBounds.top - menuHeight - gap
      : triggerBounds.bottom + gap;
    setMenuPosition({
      top: Math.min(
        Math.max(viewportPadding, requestedTop),
        Math.max(viewportPadding, window.innerHeight - menuHeight - viewportPadding),
      ),
      left: Math.min(
        Math.max(viewportPadding, triggerBounds.left),
        Math.max(viewportPadding, window.innerWidth - triggerBounds.width - viewportPadding),
      ),
      width: triggerBounds.width,
    });
  }, []);

  useLayoutEffect(() => {
    if (!open) return undefined;
    positionMenu();
    window.addEventListener("resize", positionMenu);
    document.addEventListener("scroll", positionMenu, true);
    return () => {
      window.removeEventListener("resize", positionMenu);
      document.removeEventListener("scroll", positionMenu, true);
    };
  }, [open, positionMenu]);

  useEffect(() => {
    if (!open) return undefined;
    const closeOutside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (
        !pickerRef.current?.contains(target)
        && !menuRef.current?.contains(target)
      ) {
        setOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  return (
    <>
      <fieldset ref={pickerRef} className="screenshot-text-style-picker">
        <legend>{label}</legend>
        <button
          ref={triggerRef}
          type="button"
          className="screenshot-text-style-trigger"
          aria-label={`${label}: ${selectedStyle.label}`}
          aria-haspopup="menu"
          aria-expanded={open}
          onClick={() => {
            if (open) setOpen(false);
            else {
              positionMenu();
              setOpen(true);
            }
          }}
        >
          <span
            className={`screenshot-text-style-preview style-${selectedStyle.preset}`}
            aria-hidden="true"
          >
            Text
          </span>
          <span>{selectedStyle.label}</span>
          <EditorIcon name="chevron-down" />
        </button>
      </fieldset>
      {open && createPortal(
        <div
          ref={menuRef}
          className="screenshot-text-style-menu"
          role="menu"
          aria-label={label}
          style={menuPosition}
        >
          {TEXT_STYLE_ITEMS.map((item) => (
            <button
              key={item.preset}
              type="button"
              className={value === item.preset ? "active" : ""}
              role="menuitemradio"
              aria-checked={value === item.preset}
              onClick={() => {
                onChange(item.preset);
                setOpen(false);
              }}
            >
              <span
                className={`screenshot-text-style-preview style-${item.preset}`}
                aria-hidden="true"
              >
                Text
              </span>
              <span>{item.label}</span>
            </button>
          ))}
        </div>,
        document.body,
      )}
    </>
  );
}

function CanvasBackgroundPicker({
  value,
  onChange,
}: {
  value: string | null;
  onChange: (value: string | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const [lastSolid, setLastSolid] = useState(value ?? DEFAULT_CANVAS_BACKGROUND);
  const [menuPosition, setMenuPosition] = useState({ top: 0, left: 0 });
  const pickerRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const solid = value !== null;
  if (value !== null && value !== lastSolid) {
    setLastSolid(value);
  }
  const swatchValue = value ?? lastSolid;

  const positionMenu = useCallback(() => {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const triggerBounds = trigger.getBoundingClientRect();
    const menuBounds = menuRef.current?.getBoundingClientRect();
    const menuHeight = menuBounds?.height ?? 92;
    const menuWidth = menuBounds?.width ?? 248;
    const gap = 6;
    const viewportPadding = 8;
    const roomBelow = window.innerHeight - triggerBounds.bottom - viewportPadding;
    const roomAbove = triggerBounds.top - viewportPadding;
    const openAbove = roomBelow < menuHeight && roomAbove > roomBelow;
    const requestedTop = openAbove
      ? triggerBounds.top - menuHeight - gap
      : triggerBounds.bottom + gap;
    setMenuPosition({
      top: Math.min(
        Math.max(viewportPadding, requestedTop),
        Math.max(viewportPadding, window.innerHeight - menuHeight - viewportPadding),
      ),
      left: Math.min(
        Math.max(viewportPadding, triggerBounds.left),
        Math.max(viewportPadding, window.innerWidth - menuWidth - viewportPadding),
      ),
    });
  }, []);

  useLayoutEffect(() => {
    if (!open) return undefined;
    positionMenu();
    window.addEventListener("resize", positionMenu);
    document.addEventListener("scroll", positionMenu, true);
    return () => {
      window.removeEventListener("resize", positionMenu);
      document.removeEventListener("scroll", positionMenu, true);
    };
  }, [open, positionMenu]);

  useEffect(() => {
    if (!open) return undefined;
    const closeOutside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (
        !pickerRef.current?.contains(target)
        && !menuRef.current?.contains(target)
      ) {
        setOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  return (
    <>
      <div ref={pickerRef} className="screenshot-canvas-bg">
        <button
          ref={triggerRef}
          type="button"
          className={[
            "screenshot-canvas-tool",
            "screenshot-canvas-bg-trigger",
            open ? "is-open" : "",
          ].filter(Boolean).join(" ")}
          aria-label={solid ? `Background color: ${value}` : "Background color: transparent"}
          aria-haspopup="dialog"
          aria-expanded={open}
          title="Canvas background color"
          onClick={() => {
            if (open) setOpen(false);
            else {
              positionMenu();
              setOpen(true);
            }
          }}
        >
          <span
            className={[
              "screenshot-canvas-bg-chip",
              solid ? "" : "is-transparent",
            ].filter(Boolean).join(" ")}
            style={solid ? { background: value } : undefined}
            aria-hidden="true"
          />
          Background color
          <EditorIcon name="chevron-down" />
        </button>
      </div>
      {open && createPortal(
        <div
          ref={menuRef}
          className="screenshot-canvas-bg-menu"
          role="dialog"
          aria-label="Canvas background"
          style={menuPosition}
        >
          <label className="screenshot-check-row screenshot-canvas-bg-toggle">
            <input
              type="checkbox"
              checked={solid}
              onChange={(event) => onChange(event.target.checked ? lastSolid : null)}
            />
            Solid background
          </label>
          <ColorField
            label="Canvas background"
            value={swatchValue}
            onChange={(background) => {
              setLastSolid(background);
              onChange(background);
            }}
            compact
          />
        </div>,
        document.body,
      )}
    </>
  );
}

function ColorField({
  label,
  value,
  onChange,
  compact = false,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  /** Swatches only; legend is for assistive tech. */
  compact?: boolean;
}) {
  return (
    <fieldset className={["screenshot-color-field", compact ? "compact" : ""].filter(Boolean).join(" ")}>
      <legend className={compact ? "visually-hidden" : undefined}>{label}</legend>
      <div>
        {COLOR_SWATCHES.map((color) => (
          <button
            key={color}
            type="button"
            className={value.toLowerCase().startsWith(color.toLowerCase()) ? "active" : ""}
            aria-label={`${label}: ${color}`}
            style={{ background: color }}
            onClick={() => onChange(color)}
          />
        ))}
        <label className="screenshot-custom-color" title="Custom color">
          <input type="color" value={value.slice(0, 7)} onChange={(event) => onChange(event.target.value)} />
        </label>
      </div>
    </fieldset>
  );
}

function elementLabel(element: ScreenshotElement): string {
  if (element.kind === "image") {
    return element.name;
  }
  if (element.kind === "text") return "Text";
  if (element.kind === "path") return "Freehand drawing";
  return element.shape[0].toUpperCase() + element.shape.slice(1);
}

/**
 * Elevated spill under the native drag preview. The OS image stays the only
 * floating plate; this footprint sells depth with a warm surface pool, contact
 * shadow, and thin white rim — no neon accent rays or second preview rectangle.
 */
function StackDropLight({ guide }: { guide: ImageDropGuide }) {
  const width = Math.max(1, guide.target.width);
  const height = Math.max(1, guide.target.height);
  const focusX = guide.focus.x - guide.target.x;
  const focusY = guide.focus.y - guide.target.y;
  const focusCenterX = focusX + guide.focus.width / 2;
  const focusCenterY = focusY + guide.focus.height / 2;

  return (
    <div
      className="screenshot-drop-snap-stack-light"
      style={{
        left: `${(focusX / width) * 100}%`,
        top: `${(focusY / height) * 100}%`,
        width: `${(guide.focus.width / width) * 100}%`,
        height: `${(guide.focus.height / height) * 100}%`,
      }}
      data-focus-x={focusCenterX}
      data-focus-y={focusCenterY}
      aria-hidden="true"
    >
      <i className="screenshot-drop-snap-stack-pool" />
      <i className="screenshot-drop-snap-stack-shadow" />
      <i className="screenshot-drop-snap-stack-rim" />
    </div>
  );
}

/** Short, similar-length labels so the drop toast does not reflow between modes. */
function imageDropLabel(edge: ImageDropPlacement): string {
  if (edge === "stack") return "Place on top";
  if (edge === "top") return "Place above";
  if (edge === "right") return "Place right";
  if (edge === "left") return "Place left";
  return "Place below";
}

function canvasExpandPreviewForBounds(
  bounds: EditorRect,
  canvas: Pick<ScreenshotDocument, "width" | "height">,
  element: ScreenshotElement,
): CanvasExpandPreview | null {
  const edges = canvasOverflowEdges(bounds, canvas);
  if (edges.length === 0) return null;
  const rect = previewExpandedCanvasRect(bounds, canvas);
  if (!rect) return null;
  return { edges, rect, element, canvas: { width: canvas.width, height: canvas.height } };
}

/**
 * Ghost outline glow scaled per edge: full bloom toward sides that will grow,
 * near-zero on sides that stay put (expanding edges already carry bars/particles).
 */
function canvasExpandGhostBoxShadow(edges: readonly ImageSnapEdge[]): string {
  const active = new Set(edges);
  const intensity = (edge: ImageSnapEdge): number => (active.has(edge) ? 1 : 0.08);
  const top = intensity("top");
  const bottom = intensity("bottom");
  const left = intensity("left");
  const right = intensity("right");
  return [
    "0 0 0 1px rgba(var(--theme-accent-rgb), .1)",
    // Directional glows — strength follows edge intensity (0.08 quiet → 1 full).
    `0 ${(-10 * top).toFixed(1)}px ${(18 + 22 * top).toFixed(1)}px rgba(var(--theme-accent-rgb), ${(0.04 + 0.3 * top).toFixed(3)})`,
    `0 ${(10 * bottom).toFixed(1)}px ${(18 + 22 * bottom).toFixed(1)}px rgba(var(--theme-accent-rgb), ${(0.04 + 0.3 * bottom).toFixed(3)})`,
    `${(-10 * left).toFixed(1)}px 0 ${(18 + 22 * left).toFixed(1)}px rgba(var(--theme-accent-rgb), ${(0.04 + 0.3 * left).toFixed(3)})`,
    `${(10 * right).toFixed(1)}px 0 ${(18 + 22 * right).toFixed(1)}px rgba(var(--theme-accent-rgb), ${(0.04 + 0.3 * right).toFixed(3)})`,
  ].join(", ");
}

/** Fixed particle seeds for the image-drop edge snap stream (CSS-driven). */
const DROP_SNAP_PARTICLES: Array<{
  id: string;
  /** 0–1 position along the glowing edge. */
  along: number;
  /** Relative travel multiplier for how far outward the particle flies. */
  travel: number;
  delay: string;
  duration: string;
  size: string;
}> = [
  { id: "p0", along: 0.08, travel: 0.72, delay: "0s", duration: "1.15s", size: "3px" },
  { id: "p1", along: 0.18, travel: 1.05, delay: "0.18s", duration: "1.35s", size: "2px" },
  { id: "p2", along: 0.28, travel: 0.88, delay: "0.42s", duration: "1.05s", size: "4px" },
  { id: "p3", along: 0.38, travel: 1.2, delay: "0.08s", duration: "1.45s", size: "2px" },
  { id: "p4", along: 0.48, travel: 0.95, delay: "0.55s", duration: "1.2s", size: "3px" },
  { id: "p5", along: 0.55, travel: 0.7, delay: "0.28s", duration: "0.95s", size: "2px" },
  { id: "p6", along: 0.62, travel: 1.12, delay: "0.7s", duration: "1.3s", size: "3px" },
  { id: "p7", along: 0.72, travel: 0.82, delay: "0.12s", duration: "1.1s", size: "2px" },
  { id: "p8", along: 0.8, travel: 1.28, delay: "0.48s", duration: "1.5s", size: "4px" },
  { id: "p9", along: 0.88, travel: 0.9, delay: "0.32s", duration: "1.18s", size: "2px" },
  { id: "p10", along: 0.94, travel: 0.78, delay: "0.62s", duration: "1.02s", size: "3px" },
  { id: "p11", along: 0.42, travel: 1.35, delay: "0.85s", duration: "1.4s", size: "2px" },
  { id: "p12", along: 0.15, travel: 0.65, delay: "0.95s", duration: "0.9s", size: "2px" },
  { id: "p13", along: 0.68, travel: 1.08, delay: "1.05s", duration: "1.25s", size: "3px" },
];

function elementLayerName(element: ScreenshotElement): string {
  if (element.kind === "text") {
    return element.text.trim().split("\n")[0]?.slice(0, 42) || "Text";
  }
  return elementLabel(element);
}

function elementKindLabel(element: ScreenshotElement): string {
  if (element.kind === "image") {
    return element.source === "background"
      ? element.locked ? "Locked background" : "Background"
      : "Image";
  }
  if (element.kind === "text") return "Text";
  if (element.kind === "path") return "Drawing";
  return "Shape";
}

function toolLabel(tool: ScreenshotTool): string {
  return TOOL_ITEMS.find((item) => item.tool === tool)?.label ?? "Properties";
}

function EditorIcon({ name }: { name: string }) {
  if (name === "select") return <svg viewBox="0 0 24 24"><path d="m5 3 13 9-7 2-3 7Z" /></svg>;
  if (name === "crop") return <svg viewBox="0 0 24 24"><path d="M7 3v14a2 2 0 0 0 2 2h12M3 7h14a2 2 0 0 1 2 2v12" /></svg>;
  if (name === "trim") {
    return (
      <svg viewBox="0 0 24 24">
        <rect x="8" y="8" width="8" height="8" rx="1.2" />
        <path d="M8 4H5a1 1 0 0 0-1 1v3M16 4h3a1 1 0 0 1 1 1v3M4 16v3a1 1 0 0 0 1 1h3M20 16v3a1 1 0 0 1-1 1h-3" />
      </svg>
    );
  }
  if (name === "text") return <svg viewBox="0 0 24 24"><path d="M5 5h14M12 5v14M8 19h8" /></svg>;
  if (name === "rectangle") return <svg viewBox="0 0 24 24"><rect x="4" y="5" width="16" height="14" rx="2" /></svg>;
  if (name === "ellipse") return <svg viewBox="0 0 24 24"><ellipse cx="12" cy="12" rx="8" ry="6.5" /></svg>;
  if (name === "line") return <svg viewBox="0 0 24 24"><path d="M5 19 19 5" /></svg>;
  if (name === "arrow") return <svg viewBox="0 0 24 24"><path d="M4 20 20 4M12 4h8v8" /></svg>;
  if (name === "pen") return <svg viewBox="0 0 24 24"><path d="M4 16c4-7 6-8 8-3s4 4 8-4M4 20h16" /></svg>;
  if (name === "remove-bg") {
    return (
      <svg viewBox="0 0 24 24">
        <path d="M4 20c2-6 5-9 8-9s4 2 8 9" />
        <path d="M9 7.5a2.5 2.5 0 1 0 5 0 2.5 2.5 0 0 0-5 0Z" />
        <path d="M12 10v3" />
      </svg>
    );
  }
  if (name === "rotate-counterclockwise") {
    return (
      <svg viewBox="0 0 24 24">
        <path d="M8 7H4V3" />
        <path d="M4.7 7.2A8 8 0 1 1 4 14" />
      </svg>
    );
  }
  if (name === "rotate-clockwise") {
    return (
      <svg viewBox="0 0 24 24">
        <path d="M16 7h4V3" />
        <path d="M19.3 7.2A8 8 0 1 0 20 14" />
      </svg>
    );
  }
  if (name === "flip-horizontal") {
    return (
      <svg viewBox="0 0 24 24">
        <path d="M12 3v18M9 5 4 12l5 7ZM15 5l5 7-5 7Z" />
      </svg>
    );
  }
  if (name === "flip-vertical") {
    return (
      <svg viewBox="0 0 24 24">
        <path d="M3 12h18M5 9l7-5 7 5ZM5 15l7 5 7-5Z" />
      </svg>
    );
  }
  if (name === "undo") return <svg viewBox="0 0 24 24"><path d="m9 7-5 5 5 5M5 12h8a6 6 0 0 1 6 6" /></svg>;
  if (name === "redo") return <svg viewBox="0 0 24 24"><path d="m15 7 5 5-5 5M19 12h-8a6 6 0 0 0-6 6" /></svg>;
  if (name === "fit") return <svg viewBox="0 0 24 24"><path d="M9 4H4v5M15 4h5v5M9 20H4v-5M15 20h5v-5" /></svg>;
  if (name === "image") return <svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2" /><circle cx="8" cy="9" r="1.5" /><path d="m5 18 5-5 3 3 2-2 4 4" /></svg>;
  if (name === "trash") return <svg viewBox="0 0 24 24"><path d="M4 7h16M9 7V4h6v3M7 7l1 13h8l1-13M10 11v5M14 11v5" /></svg>;
  if (name === "copy") return <svg viewBox="0 0 24 24"><rect x="8" y="8" width="11" height="11" rx="2" /><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3" /></svg>;
  if (name === "check") return <svg viewBox="0 0 24 24"><path d="m5 12 4.5 4.5L19 7" /></svg>;
  if (name === "save") return <svg viewBox="0 0 24 24"><path d="M5 3h12l2 2v16H5Z M8 3v6h8V3M8 17h8" /></svg>;
  if (name === "plus") return <svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14" /></svg>;
  if (name === "minus") return <svg viewBox="0 0 24 24"><path d="M5 12h14" /></svg>;
  if (name === "chevron-down") return <svg viewBox="0 0 24 24"><path d="m7 9 5 5 5-5" /></svg>;
  if (name === "lock") return <svg viewBox="0 0 24 24"><rect x="5" y="10" width="14" height="11" rx="2" /><path d="M8 10V7a4 4 0 0 1 8 0v3" /></svg>;
  if (name === "unlock") return <svg viewBox="0 0 24 24"><rect x="5" y="10" width="14" height="11" rx="2" /><path d="M9 10V7a4 4 0 0 1 7.5-2" /></svg>;
  if (name === "eye") return <svg viewBox="0 0 24 24"><path d="M3 12s3.5-6 9-6 9 6 9 6-3.5 6-9 6-9-6-9-6Z" /><circle cx="12" cy="12" r="2.5" /></svg>;
  if (name === "eye-off") return <svg viewBox="0 0 24 24"><path d="m4 4 16 16M9.5 6.4A9 9 0 0 1 12 6c5.5 0 9 6 9 6a15 15 0 0 1-2.2 2.9M14.4 17.6A9 9 0 0 1 12 18c-5.5 0-9-6-9-6a15 15 0 0 1 2.1-2.8" /></svg>;
  if (name === "duplicate") return <svg viewBox="0 0 24 24"><rect x="8" y="8" width="11" height="11" rx="2" /><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3M13.5 11v5M11 13.5h5" /></svg>;
  if (name === "more") {
    return (
      <svg viewBox="0 0 24 24">
        <circle cx="12" cy="5" r="1.4" />
        <circle cx="12" cy="12" r="1.4" />
        <circle cx="12" cy="19" r="1.4" />
      </svg>
    );
  }
  if (name === "bring-front") {
    return (
      <svg viewBox="0 0 24 24">
        <rect x="5" y="12" width="10" height="8" rx="1.2" opacity=".55" />
        <rect x="9" y="4" width="10" height="8" rx="1.2" />
      </svg>
    );
  }
  if (name === "send-back") {
    return (
      <svg viewBox="0 0 24 24">
        <rect x="9" y="4" width="10" height="8" rx="1.2" opacity=".55" />
        <rect x="5" y="12" width="10" height="8" rx="1.2" />
      </svg>
    );
  }
  // Layer stack tools (merge family).
  if (name === "merge-down") {
    return (
      <svg viewBox="0 0 24 24">
        <path d="M7 4h10v4H7z" />
        <path d="M12 9v5" />
        <path d="m9 12 3 3 3-3" />
        <path d="M5 17h14v3H5z" />
      </svg>
    );
  }
  if (name === "merge-visible") {
    return (
      <svg viewBox="0 0 24 24">
        <path d="M7 3h10v3H7z" />
        <path d="M7 8h10v3H7z" />
        <path d="M12 12v3" />
        <path d="m9 13.5 3 3 3-3" />
        <path d="M5 18h14v3H5z" />
      </svg>
    );
  }
  if (name === "flatten") {
    return (
      <svg viewBox="0 0 24 24">
        <path d="M6 4h12v2.5H6z" />
        <path d="M6 8.5h12v2.5H6z" />
        <path d="M6 13h12v2.5H6z" />
        <path d="M4 18h16v2.5H4z" />
      </svg>
    );
  }
  if (name === "grip") return <svg viewBox="0 0 24 24"><circle cx="9" cy="7" r=".8" /><circle cx="15" cy="7" r=".8" /><circle cx="9" cy="12" r=".8" /><circle cx="15" cy="12" r=".8" /><circle cx="9" cy="17" r=".8" /><circle cx="15" cy="17" r=".8" /></svg>;
  if (name === "align-center") return <svg viewBox="0 0 24 24"><path d="M5 6h14M8 10h8M5 14h14M8 18h8" /></svg>;
  if (name === "align-right") return <svg viewBox="0 0 24 24"><path d="M5 6h14M9 10h10M5 14h14M9 18h10" /></svg>;
  return <svg viewBox="0 0 24 24"><path d="M5 6h14M5 10h10M5 14h14M5 18h10" /></svg>;
}
