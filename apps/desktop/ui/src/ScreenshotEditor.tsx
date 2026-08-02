import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { formatFileSize } from "./lib/format";
import {
  ALIGNMENT_SNAP_SCREEN_PX,
  boundedCropRect,
  canvasOverflowEdges,
  collectAlignmentSnapLines,
  createScreenshotDocument,
  cropDocument,
  duplicateScreenshotElement,
  elementBounds,
  estimateCanvasExportBytes,
  expandDocumentForElement,
  expandDocumentToFitBounds,
  hitTestElement,
  hitTestResizeHandle,
  imageDropGuideAtPoint,
  imageSizeAtWidth,
  isSupportedImageFile,
  loadImageFile,
  outputDimensions,
  positionImportedImageAtEdge,
  reorderScreenshotLayers,
  resolveImageDropTarget,
  resizeBoundsFromHandle,
  resizeCursor,
  resizeDocumentCanvas,
  resizeElement,
  snapResizedBounds,
  snapTranslatedBounds,
  translateElement,
  type AlignmentSnapGuide,
  type EditorImageElement,
  type ImageSnapEdge,
  type LayerBlendMode,
  type LayerDropPlacement,
  type EditorPoint,
  type EditorRect,
  type ElementStyle,
  type ResizeHandle,
  type ScreenshotDocument,
  type ScreenshotElement,
  type ScreenshotTool,
  type ShapeKind,
} from "./lib/screenshotEditor";
import { NotchedSlider, RangeSlider } from "./RangeSlider";
import type { CaptureArtifact } from "./types";

type ExportFormat = "png" | "jpeg" | "webp";
type ExportSize = "original" | "75" | "50" | "custom";
type ScreenshotQuality = "70" | "85" | "92" | "97" | "100";
/** Matches the recording editor: preserve by default, compress with presets, or cap size. */
type ScreenshotQualityMode = "preserve" | "compress" | "maximum";
type ScreenshotFileSizeUnit = "kb" | "mb" | "gb";

type CachedImage = {
  image: HTMLImageElement;
  status: "loading" | "loaded" | "error";
};

type EditorGesture =
  | {
    kind: "move";
    pointerId: number;
    origin: EditorPoint;
    element: ScreenshotElement;
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
  };

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
  edge: ImageSnapEdge;
  target: EditorRect;
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
  { tool: "curved_arrow", label: "Curved arrow", shortcut: "B" },
  { tool: "pen", label: "Freehand", shortcut: "P" },
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

const SCREENSHOT_QUALITY_OPTIONS = [
  { value: "70", label: "Smaller", shortLabel: "Small" },
  { value: "85", label: "Balanced", shortLabel: "Medium" },
  { value: "92", label: "High" },
  { value: "97", label: "Very high", shortLabel: "V. high" },
  { value: "100", label: "Maximum", shortLabel: "Max" },
] as const;

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
  return "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";
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

function arrowHead(
  context: CanvasRenderingContext2D,
  end: EditorPoint,
  tangent: EditorPoint,
  strokeWidth: number,
): void {
  const angle = Math.atan2(end.y - tangent.y, end.x - tangent.x);
  const length = Math.max(14, strokeWidth * 4.2);
  context.beginPath();
  context.moveTo(end.x, end.y);
  context.lineTo(
    end.x - length * Math.cos(angle - Math.PI / 6),
    end.y - length * Math.sin(angle - Math.PI / 6),
  );
  context.moveTo(end.x, end.y);
  context.lineTo(
    end.x - length * Math.cos(angle + Math.PI / 6),
    end.y - length * Math.sin(angle + Math.PI / 6),
  );
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

  if (shape === "curved_arrow") {
    const midpoint = { x: (x + endX) / 2, y: (y + endY) / 2 };
    const delta = { x: endX - x, y: endY - y };
    const control = {
      x: midpoint.x - delta.y * element.bend,
      y: midpoint.y + delta.x * element.bend,
    };
    context.beginPath();
    context.moveTo(x, y);
    context.quadraticCurveTo(control.x, control.y, endX, endY);
    context.stroke();
    arrowHead(context, { x: endX, y: endY }, control, style.strokeWidth);
    context.restore();
    return;
  }

  context.beginPath();
  context.moveTo(x, y);
  context.lineTo(endX, endY);
  context.stroke();
  if (shape === "arrow") {
    arrowHead(context, { x: endX, y: endY }, { x, y }, style.strokeWidth);
  }
  context.restore();
}

function drawText(
  context: CanvasRenderingContext2D,
  element: Extract<ScreenshotElement, { kind: "text" }>,
): void {
  const lines = element.text.split("\n");
  const lineHeight = element.fontSize * 1.25;
  context.save();
  context.font = [
    element.italic ? "italic" : "",
    element.bold ? "700" : "400",
    `${element.fontSize}px`,
    fontFamily(element),
  ].filter(Boolean).join(" ");
  context.textBaseline = "top";
  context.textAlign = element.align;
  const width = Math.max(
    element.fontSize,
    ...lines.map((line) => context.measureText(line || " ").width),
  );
  const anchorX = element.align === "center"
    ? element.x + width / 2
    : element.align === "right" ? element.x + width : element.x;
  if (element.background) {
    context.fillStyle = element.background;
    context.fillRect(
      element.x - element.fontSize * 0.18,
      element.y - element.fontSize * 0.12,
      width + element.fontSize * 0.36,
      lines.length * lineHeight + element.fontSize * 0.14,
    );
  }
  context.fillStyle = element.color;
  lines.forEach((line, index) => {
    context.fillText(line || " ", anchorX, element.y + index * lineHeight);
  });
  context.restore();
}

function renderScreenshot(
  context: CanvasRenderingContext2D,
  document: ScreenshotDocument,
  imageCache: Map<string, CachedImage>,
): void {
  context.clearRect(0, 0, document.width, document.height);
  if (document.background) {
    context.fillStyle = document.background;
    context.fillRect(0, 0, document.width, document.height);
  }
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = "high";
  for (const element of document.elements) {
    if (!element.visible) continue;
    context.save();
    context.globalAlpha = Math.max(0, Math.min(1, element.opacity / 100));
    context.globalCompositeOperation = element.blendMode;
    if (element.kind === "image") {
      const cached = imageCache.get(element.src);
      if (cached?.status === "loaded") {
        context.drawImage(
          cached.image,
          element.x,
          element.y,
          element.width,
          element.height,
        );
      }
    } else if (element.kind === "text") {
      drawText(context, element);
    } else if (element.kind === "shape") {
      drawShape(context, element);
    } else {
      context.save();
      context.strokeStyle = element.style.color;
      context.lineWidth = element.style.strokeWidth;
      context.lineCap = "round";
      context.lineJoin = "round";
      drawSmoothPath(context, element.points);
      context.restore();
    }
    context.restore();
  }
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
      context.save();
      context.strokeStyle = accentColor;
      context.lineWidth = 2 * unit;
      context.setLineDash([7 * unit, 5 * unit]);
      context.strokeRect(bounds.x, bounds.y, bounds.width, bounds.height);
      context.setLineDash([]);
      context.fillStyle = accentColor;
      for (const point of [
        [bounds.x, bounds.y],
        [bounds.x + bounds.width, bounds.y],
        [bounds.x + bounds.width, bounds.y + bounds.height],
        [bounds.x, bounds.y + bounds.height],
      ]) {
        context.fillRect(point[0] - 4 * unit, point[1] - 4 * unit, 8 * unit, 8 * unit);
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
  const [cropSelection, setCropSelection] = useState<EditorRect | null>(null);
  const [cropAspect, setCropAspect] = useState("free");
  const [defaultStyle, setDefaultStyle] = useState<ElementStyle>({
    color: "#ff3b5c",
    fill: null,
    strokeWidth: 8,
  });
  const [defaultFontSize, setDefaultFontSize] = useState(48);
  const [fitScale, setFitScale] = useState(1);
  const [zoomMode, setZoomMode] = useState<"fit" | "manual">("fit");
  const [zoom, setZoom] = useState(100);
  const [imageRevision, setImageRevision] = useState(0);
  const [dragActive, setDragActive] = useState(false);
  const [imageDropGuide, setImageDropGuide] = useState<ImageDropGuide | null>(null);
  const imageDropGuideRef = useRef<ImageDropGuide | null>(null);
  const [draggedLayerId, setDraggedLayerId] = useState<string | null>(null);
  const [resizePreviewBounds, setResizePreviewBounds] = useState<EditorRect | null>(null);
  const [alignmentGuides, setAlignmentGuides] = useState<AlignmentSnapGuide[]>([]);
  const [canvasExpandEdges, setCanvasExpandEdges] = useState<ImageSnapEdge[]>([]);
  const [canvasCursor, setCanvasCursor] = useState<string | undefined>(undefined);
  const [layerDropTarget, setLayerDropTarget] = useState<LayerDropTarget | null>(null);
  const [exportFormat, setExportFormat] = useState<ExportFormat>("png");
  const [exportSize, setExportSize] = useState<ExportSize>("original");
  const [customExportWidth, setCustomExportWidth] = useState(1_920);
  const [customExportHeight, setCustomExportHeight] = useState(1_080);
  const [exportAspectLocked, setExportAspectLocked] = useState(true);
  const [jpegQuality, setJpegQuality] = useState<ScreenshotQuality>("100");
  const [qualityMode, setQualityMode] =
    useState<ScreenshotQualityMode>("preserve");
  const [maximumFileSize, setMaximumFileSize] = useState("10");
  const [maximumFileSizeUnit, setMaximumFileSizeUnit] =
    useState<ScreenshotFileSizeUnit>("mb");
  const [filenameStem, setFilenameStem] = useState("");
  const [destinationDirectory, setDestinationDirectory] = useState("");
  const [estimatedBytes, setEstimatedBytes] = useState<number | null>(null);
  const [estimatePending, setEstimatePending] = useState(false);
  const baselineDocumentRef = useRef<ScreenshotDocument | null>(null);
  const [busy, setBusy] = useState<"copying" | "saving" | null>(null);
  /** Transient success for copy/save — does not replace the stable export hint. */
  const [success, setSuccess] = useState<{ kind: "copy" | "save"; message: string } | null>(null);
  const [error, setError] = useState("");
  /** Original capture was deleted after the editor opened; the edit is still exportable. */
  const [sourceMissing, setSourceMissing] = useState(false);
  const [makeCopy, setMakeCopy] = useState(false);
  const [saved, setSaved] = useState<SavedScreenshotEdit | null>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const imageCacheRef = useRef(new Map<string, CachedImage>());
  const successTimerRef = useRef<number | null>(null);
  const objectUrlsRef = useRef(new Set<string>());
  const gestureRef = useRef<EditorGesture | null>(null);
  const dropDepthRef = useRef(0);
  const displayedZoomPercentRef = useRef(100);
  const zoomAnchorFrameRef = useRef<number | null>(null);
  const magnifyGestureRef = useRef<{
    initialZoomPercent: number;
    clientX: number;
    clientY: number;
  } | null>(null);

  const updateDocument = useCallback((
    updater: (current: ScreenshotDocument) => ScreenshotDocument,
  ) => {
    setEditorDocument((current) => {
      if (!current) return current;
      const next = updater(current);
      documentRef.current = next;
      return next;
    });
  }, []);

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
  }, []);

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
      const next = createScreenshotDocument(
        loaded.full_url,
        loaded.width,
        loaded.height,
      );
      ensureImage(loaded.full_url);
      setArtifact(loaded);
      baselineDocumentRef.current = next;
      replaceDocument(next);
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

  const selected = useMemo(() => (
    editorDocument?.elements.find((element) => element.id === selectedId) ?? null
  ), [editorDocument, selectedId]);

  const displayScale = zoomMode === "fit" ? fitScale : zoom / 100;

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
      viewport.scrollLeft += nextClientX - anchor.clientX;
      viewport.scrollTop += nextClientY - anchor.clientY;
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
    setZoomMode("fit");
  }, []);

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

  useEffect(() => {
    if (!editorDocument || !canvasRef.current) return;
    editorDocument.elements
      .filter((element): element is EditorImageElement => element.kind === "image")
      .forEach((element) => ensureImage(element.src));
    const canvas = canvasRef.current;
    if (canvas.width !== editorDocument.width) canvas.width = editorDocument.width;
    if (canvas.height !== editorDocument.height) canvas.height = editorDocument.height;
    const context = canvas.getContext("2d");
    if (!context) return;
    renderScreenshot(context, editorDocument, imageCacheRef.current);
    const accentColor = getComputedStyle(canvas)
      .getPropertyValue("--theme-accent")
      .trim() || "#ffffff";
    drawEditorOverlays(
      context,
      editorDocument,
      selected,
      cropSelection,
      displayScale,
      accentColor,
      resizePreviewBounds,
    );
  }, [
    cropSelection,
    displayScale,
    editorDocument,
    resizePreviewBounds,
    ensureImage,
    imageRevision,
    selected,
  ]);

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

  const deleteSelected = useCallback(() => {
    const current = documentRef.current;
    const element = current?.elements.find(({ id }) => id === selectedId);
    if (!current || !element || element.locked) return;
    commitDocument({
      ...current,
      elements: current.elements.filter(({ id }) => id !== selectedId),
    });
    setSelectedId(null);
  }, [commitDocument, selectedId]);

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

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const command = event.metaKey || event.ctrlKey;
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
      const target = event.target as HTMLElement | null;
      if (target?.matches("input, textarea, select, [contenteditable=true]")) return;
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
          setTool(match.tool);
          if (match.tool !== "select") setSelectedId(null);
          setCropSelection(null);
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [deleteSelected, nudgeSelected, redo, setManualZoom, undo, zoomBy]);

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

  const canvasPoint = (event: React.PointerEvent<HTMLCanvasElement>): EditorPoint => {
    const canvas = event.currentTarget;
    const bounds = canvas.getBoundingClientRect();
    return {
      x: (event.clientX - bounds.left) * canvas.width / Math.max(1, bounds.width),
      y: (event.clientY - bounds.top) * canvas.height / Math.max(1, bounds.height),
    };
  };

  const startPointer = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const current = documentRef.current;
    if (!current || event.button !== 0) return;
    const point = canvasPoint(event);
    setError("");
    clearSuccess();
    setSaved(null);

    if (tool === "select") {
      const handleRadius = 10 / Math.max(0.01, displayScale);
      const selectedElement = selectedId
        ? current.elements.find((element) => element.id === selectedId) ?? null
        : null;
      if (
        selectedElement
        && !selectedElement.locked
      ) {
        const bounds = elementBounds(selectedElement);
        const handle = hitTestResizeHandle(bounds, point, handleRadius);
        if (handle) {
          gestureRef.current = {
            kind: "resize",
            pointerId: event.pointerId,
            handle,
            element: selectedElement,
            initialBounds: bounds,
            currentBounds: bounds,
            initialDocument: current,
          };
          setResizePreviewBounds(bounds);
          setCanvasCursor(resizeCursor(handle));
          event.currentTarget.setPointerCapture(event.pointerId);
          return;
        }
      }

      const element = hitTestElement(current.elements, point, handleRadius);
      setSelectedId(element?.id ?? null);
      if (element) {
        gestureRef.current = {
          kind: "move",
          pointerId: event.pointerId,
          origin: point,
          element,
          initialDocument: current,
        };
        setCanvasCursor("move");
        event.currentTarget.setPointerCapture(event.pointerId);
      } else {
        setCanvasCursor(undefined);
      }
      return;
    }

    if (tool === "crop") {
      setSelectedId(null);
      setCropSelection({ x: point.x, y: point.y, width: 1, height: 1 });
      gestureRef.current = {
        kind: "crop",
        pointerId: event.pointerId,
        origin: point,
      };
      event.currentTarget.setPointerCapture(event.pointerId);
      return;
    }

    if (tool === "text") {
      const element: ScreenshotElement = {
        id: editorId(),
        kind: "text",
        x: point.x,
        y: point.y,
        text: "Text",
        fontSize: defaultFontSize,
        fontFamily: "sans",
        bold: false,
        italic: false,
        align: "left",
        color: defaultStyle.color,
        background: null,
        locked: false,
        visible: true,
        opacity: 100,
        blendMode: "source-over",
      };
      commitDocument({ ...current, elements: [...current.elements, element] });
      setSelectedId(element.id);
      setTool("select");
      return;
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
        bend: 0.24,
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
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const movePointer = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const gesture = gestureRef.current;
    const point = canvasPoint(event);
    if (!gesture || gesture.pointerId !== event.pointerId) {
      if (tool === "select") {
        const current = documentRef.current;
        const selectedElement = selectedId && current
          ? current.elements.find((element) => element.id === selectedId) ?? null
          : null;
        if (
          selectedElement
          && !selectedElement.locked
        ) {
          const handle = hitTestResizeHandle(
            elementBounds(selectedElement),
            point,
            10 / Math.max(0.01, displayScale),
          );
          if (handle) {
            setCanvasCursor(resizeCursor(handle));
            return;
          }
        }
        const hovered = current
          ? hitTestElement(current.elements, point, 10 / Math.max(0.01, displayScale))
          : null;
        setCanvasCursor(hovered ? "move" : undefined);
        return;
      }
      setCanvasCursor(undefined);
      return;
    }
    if (gesture.kind === "crop") {
      const aspectRatio = cropAspect === "free"
        ? null
        : cropAspect.split(":").map(Number).reduce((width, height) => width / height);
      setCropSelection(boundedCropRect(
        gesture.origin,
        point,
        documentRef.current ?? { width: 1, height: 1 },
        aspectRatio,
      ));
      return;
    }
    if (gesture.kind === "move") {
      setCanvasCursor("move");
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
      setCanvasExpandEdges(canvasOverflowEdges(
        elementBounds(moved),
        gesture.initialDocument,
      ));
      replaceDocument(nextDocument);
      return;
    }
    if (gesture.kind === "resize") {
      setCanvasCursor(resizeCursor(gesture.handle));
      const minSize = 8 / Math.max(0.01, displayScale);
      const snapThreshold = ALIGNMENT_SNAP_SCREEN_PX / Math.max(0.01, displayScale);
      const freeBounds = resizeBoundsFromHandle(
        gesture.initialBounds,
        gesture.handle,
        point,
        minSize,
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
      const resized = resizeElement(
        gesture.element,
        gesture.initialBounds,
        snapped.bounds,
      );
      gestureRef.current = { ...gesture, currentBounds: snapped.bounds };
      setResizePreviewBounds(snapped.bounds);
      setAlignmentGuides(snapped.guides);
      setCanvasExpandEdges(canvasOverflowEdges(
        snapped.bounds,
        gesture.initialDocument,
      ));
      replaceDocument(replaceElement(
        gesture.initialDocument,
        gesture.element.id,
        resized,
      ));
      return;
    }
    updateDocument((current) => {
      const element = current.elements.find(({ id }) => id === gesture.elementId);
      if (!element) return current;
      if (element.kind === "path") {
        const last = element.points.at(-1);
        if (last && Math.hypot(point.x - last.x, point.y - last.y) < 1.5 / displayScale) {
          return current;
        }
        return replaceElement(current, element.id, {
          ...element,
          points: [...element.points, point],
        });
      }
      if (element.kind === "shape") {
        return replaceElement(current, element.id, {
          ...element,
          endX: point.x,
          endY: point.y,
        });
      }
      return current;
    });
  };

  const finishPointer = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const gesture = gestureRef.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    gestureRef.current = null;
    setResizePreviewBounds(null);
    setAlignmentGuides([]);
    setCanvasExpandEdges([]);
    setCanvasCursor(undefined);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (gesture.kind === "crop") return;
    let current = documentRef.current;
    if (!current) return;

    // Dragging a layer past the canvas edge expands the document on release.
    if (gesture.kind === "resize" || gesture.kind === "move") {
      const element = current.elements.find(({ id }) => id === gesture.element.id);
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

    if (JSON.stringify(current) === JSON.stringify(gesture.initialDocument)) return;
    setUndoStack((stack) => [...stack.slice(-99), gesture.initialDocument]);
    setRedoStack([]);
  };

  const applyCrop = () => {
    const current = documentRef.current;
    if (!current || !cropSelection) return;
    commitDocument(cropDocument(current, cropSelection));
    setCropSelection(null);
    setSelectedId(null);
    setTool("select");
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

  const duplicateSelected = () => {
    const current = documentRef.current;
    const index = current?.elements.findIndex(({ id }) => id === selectedId) ?? -1;
    if (!current || index < 0) return;
    const duplicate = duplicateScreenshotElement(current.elements[index], editorId());
    const elements = [...current.elements];
    elements.splice(index + 1, 0, duplicate);
    commitDocument({ ...current, elements });
    setSelectedId(duplicate.id);
    setTool("select");
  };

  const moveLayer = (direction: "front" | "back") => {
    const current = documentRef.current;
    if (!current || !selectedId) return;
    const index = current.elements.findIndex(({ id }) => id === selectedId);
    if (index < 0 || current.elements[index].locked) return;
    const target = direction === "front"
      ? current.elements.at(-1)
      : current.elements[0];
    if (!target || target.id === selectedId) return;
    const elements = reorderScreenshotLayers(
      current.elements,
      selectedId,
      target.id,
      direction === "front" ? "before" : "after",
    );
    if (elements === current.elements) return;
    commitDocument({ ...current, elements });
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

  const defaultImageDropGuide = (current: ScreenshotDocument): ImageDropGuide => ({
    // Used only when a drop arrives before any drag-over pointer sample.
    edge: "bottom",
    target: resolveImageDropTarget(current, selectedId),
  });

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
        );
        const element: EditorImageElement = {
          id: editorId(),
          kind: "image",
          source: "imported",
          src: image.src,
          name: file.name,
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
        next = expandDocumentForElement(next, element);
        lastId = element.id;
        const added = next.elements.find(({ id }) => id === element.id);
        if (added) {
          placement = { edge: placement.edge, target: elementBounds(added) };
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
      const estimateFormat: ExportFormat = qualityMode === "preserve"
        ? exportFormat
        : "jpeg";
      if (shouldUseOriginalFileSizeEstimate(
        artifact,
        editorDocument,
        baselineDocumentRef.current,
        estimateFormat,
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
          const estimateQuality = qualityMode === "preserve"
            ? 100
            : Number(jpegQuality);
          const bytes = await estimateCanvasExportBytes(
            canvas,
            estimateFormat,
            estimateQuality,
          );
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
    // Size targeting and quality presets need a lossy codec (JPEG).
    const saveFormat: ExportFormat = qualityMode === "preserve" ? exportFormat : "jpeg";
    const saveQuality = qualityMode === "preserve" ? 100 : Number(jpegQuality);
    if (saveFormat !== exportFormat) {
      setExportFormat(saveFormat);
    }
    setBusy("saving");
    setError("");
    clearSuccess();
    try {
      const destinationPath = screenshotDestinationPath(
        destinationDirectory,
        filenameStem,
        saveFormat,
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
          format: saveFormat,
          jpeg_quality: saveQuality,
          max_size_bytes: maximumSizeBytes,
          overwrite_source: overwriteSource,
          image_png: imagePng,
        },
      });
      if (overwriteSource) {
        setArtifact(result.artifact);
        if (documentRef.current) {
          baselineDocumentRef.current = documentRef.current;
        }
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
  // Compress and maximum-size modes always encode as JPEG so quality/size limits work.
  const effectiveExportFormat: ExportFormat = qualityMode === "preserve"
    ? exportFormat
    : "jpeg";
  const formatRequiresCopy = sourceMissing
    || !screenshotPathMatchesFormat(artifact.path, effectiveExportFormat);
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
        ? `≤ ${formatFileSize(maximumSizeBytes)}`
        : `≈ ${formatFileSize(estimatedBytes)}`;

  const applyExportFormat = (format: ExportFormat) => {
    setExportFormat(format);
    // Lossless codecs stay preserve-only; compression switches the mode off.
    if (format !== "jpeg" && qualityMode !== "preserve") {
      setQualityMode("preserve");
    }
    setSaved(null);
    clearSuccess();
  };

  const applyQualityMode = (mode: ScreenshotQualityMode) => {
    setQualityMode(mode);
    // Quality presets and hard size caps need JPEG (PNG/WebP stay lossless).
    if (mode !== "preserve" && exportFormat !== "jpeg") {
      setExportFormat("jpeg");
    }
    setSaved(null);
    clearSuccess();
  };

  const updateMakeCopy = (enabled: boolean) => {
    if (formatRequiresCopy) return;
    setMakeCopy(enabled);
    if (enabled && filenameStem === sourceStem && destinationDirectory === sourceDirectory) {
      setFilenameStem(`${sourceStem}-copy`);
    } else if (!enabled) {
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
        }
      }}
      onDrop={(event) => {
        if (!isFileTransfer(event.dataTransfer)) return;
        event.preventDefault();
        dropDepthRef.current = 0;
        setDragActive(false);
        // Prefer the latest pointer sample from dragover; React state can lag.
        const guide = imageDropGuideRef.current
          ?? imageDropGuide
          ?? defaultImageDropGuide(editorDocument);
        setImageDropGuideState(null);
        void loadDroppedFiles(Array.from(event.dataTransfer.files), guide);
      }}
    >
      <header className="screenshot-editor-header">
        <div>
          <span>Screenshot editor</span>
          <strong>{editorDocument.width} × {editorDocument.height}</strong>
        </div>
        <div className="screenshot-editor-history-actions">
          <button type="button" disabled={undoStack.length === 0} onClick={undo} aria-label="Undo">
            <EditorIcon name="undo" />
          </button>
          <button type="button" disabled={redoStack.length === 0} onClick={redo} aria-label="Redo">
            <EditorIcon name="redo" />
          </button>
          <span className="screenshot-editor-zoom">
            <button
              type="button"
              className={zoomMode === "fit" ? "active" : ""}
              onClick={activateFitZoom}
            >
              Fit
            </button>
            <select
              aria-label="Canvas zoom"
              title="Pinch or use Command/Ctrl + or - to zoom"
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
        className="screenshot-canvas-viewport"
        aria-label="Screenshot editing canvas"
      >
        <div
          className={[
            "screenshot-canvas-surface",
            editorDocument.background ? "" : "transparent",
          ].filter(Boolean).join(" ")}
          style={{
            width: editorDocument.width * displayScale,
            height: editorDocument.height * displayScale,
            backgroundColor: editorDocument.background ?? undefined,
          }}
        >
          <canvas
            ref={canvasRef}
            width={editorDocument.width}
            height={editorDocument.height}
            style={{
              width: editorDocument.width * displayScale,
              height: editorDocument.height * displayScale,
              cursor: canvasCursor,
            }}
            className={`screenshot-canvas tool-${tool}`}
            onPointerDown={startPointer}
            onPointerMove={movePointer}
            onPointerUp={finishPointer}
            onPointerCancel={finishPointer}
          />
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
              <span>{imageDropLabel(imageDropGuide.edge)}</span>
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
          {canvasExpandEdges.length > 0 && (
            <div
              className={[
                "screenshot-canvas-expand-hint",
                ...canvasExpandEdges.map((edge) => `edge-${edge}`),
              ].join(" ")}
              aria-hidden="true"
            >
              <span>Release to expand canvas</span>
            </div>
          )}
        </div>
        {dragActive && (
          <div className="screenshot-drop-overlay" aria-hidden="true">
            <EditorIcon name="image" />
            <strong>{imageDropGuide ? imageDropLabel(imageDropGuide.edge) : "Drop image"}</strong>
            <span>It will snap to the highlighted edge and stay editable.</span>
          </div>
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
                  draggable={!locked}
                  onDragStart={(event) => {
                    if (locked) {
                      event.preventDefault();
                      return;
                    }
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
                  <button
                    type="button"
                    className="screenshot-layer-select"
                    aria-pressed={selectedId === element.id}
                    onClick={() => {
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
                        ? <img src={element.src} alt="" draggable={false} />
                        : <EditorIcon name={layerIconName(element)} />}
                    </span>
                    <span className="screenshot-layer-copy">
                      <strong>{elementLayerName(element)}</strong>
                      <small>{elementKindLabel(element)}</small>
                    </span>
                  </button>
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
                        updateLayer(element.id, (current) => ({
                          ...current,
                          locked: !current.locked,
                        }));
                        setSelectedId(element.id);
                      }}
                    >
                      <EditorIcon name={locked ? "lock" : "unlock"} />
                    </button>
                  </span>
                </li>
              );
            })}
          </ol>
        </section>

        <section className="screenshot-properties" aria-label="Tool properties">
        <div className="screenshot-properties-heading">
          <strong>{selected ? elementLabel(selected) : toolLabel(tool)}</strong>
          {selected && !selected.locked && (
            <button type="button" aria-label="Delete selected item" onClick={deleteSelected}>
              <EditorIcon name="trash" />
            </button>
          )}
        </div>

        {selected && (
          <section className="screenshot-property-section screenshot-layer-inspector">
            {selected.kind === "image" && (
              <label>
                Layer name
                <input
                  value={selected.name}
                  onChange={(event) => updateSelected((element) => (
                    element.kind === "image" ? { ...element, name: event.target.value } : element
                  ))}
                />
              </label>
            )}
            <label>
              Blend mode
              <select
                value={selected.blendMode}
                onChange={(event) => updateSelected((element) => ({
                  ...element,
                  blendMode: event.target.value as LayerBlendMode,
                }))}
              >
                {LAYER_BLEND_MODE_OPTIONS.map((option) => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
            </label>
            <label>
              Opacity
              <RangeSlider
                ariaLabel="Layer opacity"
                min={0}
                max={100}
                value={selected.opacity}
                valueText={`${selected.opacity}%`}
                marks={[
                  { value: 0, label: "0" },
                  { value: 50, label: "50" },
                  { value: 100, label: "100" },
                ]}
                onChange={(opacity) => updateSelected((element) => ({ ...element, opacity }))}
              />
            </label>
            <div className="screenshot-layer-state-grid">
              <button
                type="button"
                className={selected.locked ? "active" : ""}
                aria-pressed={selected.locked}
                onClick={() => updateSelected((element) => ({
                  ...element,
                  locked: !element.locked,
                }))}
              >
                <EditorIcon name={selected.locked ? "lock" : "unlock"} />
                {selected.locked ? "Locked" : "Unlocked"}
              </button>
              <button
                type="button"
                className={selected.visible ? "active" : ""}
                aria-pressed={selected.visible}
                onClick={() => updateSelected((element) => ({
                  ...element,
                  visible: !element.visible,
                }))}
              >
                <EditorIcon name={selected.visible ? "eye" : "eye-off"} />
                {selected.visible ? "Visible" : "Hidden"}
              </button>
            </div>
            <div className="screenshot-layer-action-grid">
              <button type="button" onClick={duplicateSelected}>
                <EditorIcon name="duplicate" />Duplicate
              </button>
              <button type="button" disabled={selected.locked} onClick={deleteSelected}>
                <EditorIcon name="trash" />Delete
              </button>
            </div>
          </section>
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
                  <label>Width<input value={cropSelection.width} readOnly /></label>
                  <label>Height<input value={cropSelection.height} readOnly /></label>
                </div>
                <div className="screenshot-property-actions">
                  <button type="button" onClick={() => setCropSelection(null)}>Clear</button>
                  <button type="button" className="primary" onClick={applyCrop}>Apply crop</button>
                </div>
              </>
            ) : (
              <p>Drag over the area you want to keep.</p>
            )}
          </section>
        )}

        {selected?.kind === "text" && (
          <section className="screenshot-property-section">
            <label>
              Text
              <textarea
                autoFocus
                rows={4}
                value={selected.text}
                onChange={(event) => updateSelected((element) => (
                  element.kind === "text" ? { ...element, text: event.target.value } : element
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
                      ? { ...element, fontFamily: event.target.value as typeof element.fontFamily }
                      : element
                  ))}
                >
                  <option value="sans">Sans serif</option>
                  <option value="serif">Serif</option>
                  <option value="mono">Monospace</option>
                </select>
              </label>
              <label>
                Size
                <input
                  type="number"
                  min={8}
                  max={512}
                  value={selected.fontSize}
                  onChange={(event) => updateSelected((element) => (
                    element.kind === "text"
                      ? { ...element, fontSize: Math.max(8, Number(event.target.value)) }
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
                  element.kind === "text" ? { ...element, bold: !element.bold } : element
                ))}
              >B</button>
              <button
                type="button"
                className={selected.italic ? "active" : ""}
                aria-label="Italic"
                onClick={() => updateSelected((element) => (
                  element.kind === "text" ? { ...element, italic: !element.italic } : element
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
                    ? { ...element, background: event.target.checked ? "#111318" : null }
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
                <input
                  type="number"
                  min={1}
                  max={16_384}
                  value={Math.round(selected.width)}
                  disabled={selected.locked}
                  onChange={(event) => updateSelected((element) => {
                    if (element.kind !== "image") return element;
                    const size = imageSizeAtWidth(element, Number(event.target.value));
                    return { ...element, ...size };
                  })}
                />
              </label>
              <label>Height<input value={Math.round(selected.height)} readOnly /></label>
              <label>
                X
                <input
                  type="number"
                  value={Math.round(selected.x)}
                  disabled={selected.locked}
                  onChange={(event) => updateSelected((element) => (
                    element.kind === "image" ? { ...element, x: Number(event.target.value) } : element
                  ))}
                />
              </label>
              <label>
                Y
                <input
                  type="number"
                  value={Math.round(selected.y)}
                  disabled={selected.locked}
                  onChange={(event) => updateSelected((element) => (
                    element.kind === "image" ? { ...element, y: Number(event.target.value) } : element
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
                marks={[
                  { value: 2, label: "2" },
                  { value: 12, label: "12" },
                  { value: 24, label: "24" },
                  { value: 40, label: "40" },
                ]}
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
            {selected.kind === "shape" && selected.shape === "curved_arrow" && (
              <label>
                Curve
                <RangeSlider
                  ariaLabel="Curve"
                  min={-50}
                  max={50}
                  value={Math.round(selected.bend * 100)}
                  valueText={`${Math.round(selected.bend * 100)}%`}
                  marks={[
                    { value: -50, label: "Left" },
                    { value: 0, label: "Straight" },
                    { value: 50, label: "Right" },
                  ]}
                  onChange={(bend) => updateSelected((element) => (
                    element.kind === "shape"
                      ? { ...element, bend: bend / 100 }
                      : element
                  ))}
                />
              </label>
            )}
          </section>
        )}

        {!selected && tool !== "crop" && (
          <section className="screenshot-property-section">
            {tool === "text" ? (
              <label>
                New text size
                <input
                  type="number"
                  min={8}
                  max={512}
                  value={defaultFontSize}
                  onChange={(event) => setDefaultFontSize(Number(event.target.value))}
                />
              </label>
            ) : tool !== "select" ? (
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
                    marks={[
                      { value: 2, label: "2" },
                      { value: 12, label: "12" },
                      { value: 24, label: "24" },
                      { value: 40, label: "40" },
                    ]}
                    onChange={(strokeWidth) => setDefaultStyle((style) => ({
                      ...style,
                      strokeWidth,
                    }))}
                  />
                </label>
              </>
            ) : (
              <p>Select an annotation or imported image to move and format it.</p>
            )}
          </section>
        )}

        {selected && !selected.locked && (
          <section className="screenshot-property-section screenshot-layer-actions">
            <button type="button" onClick={() => moveLayer("front")}>Bring to front</button>
            <button type="button" onClick={() => moveLayer("back")}>Send to back</button>
          </section>
        )}

        <section className="screenshot-property-section">
          <h2>Canvas</h2>
          <div className="screenshot-number-pair">
            <label>
              Width
              <input
                type="number"
                min={1}
                max={16_384}
                value={editorDocument.width}
                onChange={(event) => commitDocument(resizeDocumentCanvas(
                  editorDocument,
                  Number(event.target.value),
                  editorDocument.height,
                ))}
              />
            </label>
            <label>
              Height
              <input
                type="number"
                min={1}
                max={16_384}
                value={editorDocument.height}
                onChange={(event) => commitDocument(resizeDocumentCanvas(
                  editorDocument,
                  editorDocument.width,
                  Number(event.target.value),
                ))}
              />
            </label>
          </div>
          <label className="screenshot-check-row">
            <input
              type="checkbox"
              checked={editorDocument.background !== null}
              onChange={(event) => commitDocument({
                ...editorDocument,
                background: event.target.checked ? "#f7f7f5" : null,
              })}
            />
            Solid canvas background
          </label>
          {editorDocument.background !== null && (
            <ColorField
              label="Canvas background"
              value={editorDocument.background}
              onChange={(background) => commitDocument({ ...editorDocument, background })}
            />
          )}
        </section>
        </section>
      </aside>

      <footer className="screenshot-export-bar">
        <div className="screenshot-export-settings">
          <label>
            Format
            <select
              value={effectiveExportFormat}
              onChange={(event) => applyExportFormat(event.target.value as ExportFormat)}
            >
              <option value="png">PNG · lossless</option>
              <option value="jpeg">JPEG</option>
              <option value="webp">WebP · lossless</option>
            </select>
          </label>
          <label className="screenshot-export-size">
            Output size
            <span className="screenshot-export-size-control">
              <select value={exportSize} onChange={(event) => {
                const next = event.target.value as ExportSize;
                if (next === "custom" && exportSize !== "custom") {
                  setCustomExportWidth(editorDocument.width);
                  setCustomExportHeight(editorDocument.height);
                }
                setExportSize(next);
              }}>
                <option value="original">Original</option>
                <option value="75">75%</option>
                <option value="50">50%</option>
                <option value="custom">Custom</option>
              </select>
              <span className="screenshot-output-dimensions" aria-live="polite">
                {output.width} × {output.height}
              </span>
            </span>
          </label>
          {exportSize === "custom" && (
            <div className="screenshot-export-control screenshot-custom-dimensions">
              <span>Width × height</span>
              <div>
                <input
                  type="number"
                  min={1}
                  max={MAX_SCREENSHOT_OUTPUT_DIMENSION}
                  value={customExportWidth}
                  aria-label="Custom output width"
                  onChange={(event) => updateCustomExportDimension("width", Number(event.target.value))}
                />
                <span aria-hidden="true">×</span>
                <input
                  type="number"
                  min={1}
                  max={MAX_SCREENSHOT_OUTPUT_DIMENSION}
                  value={customExportHeight}
                  aria-label="Custom output height"
                  onChange={(event) => updateCustomExportDimension("height", Number(event.target.value))}
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
          <label className="screenshot-quality-mode">
            Save quality
            <select
              aria-label="Save quality"
              value={qualityMode}
              onChange={(event) => {
                applyQualityMode(event.target.value as ScreenshotQualityMode);
              }}
            >
              <option value="preserve">Preserve quality</option>
              <option value="compress">Compress</option>
              <option value="maximum">Maximum file size</option>
            </select>
          </label>
          {qualityMode === "compress" && (
            <div className="screenshot-export-control screenshot-quality">
              <span>Compression quality</span>
              <NotchedSlider
                ariaLabel="Image quality"
                value={jpegQuality}
                options={SCREENSHOT_QUALITY_OPTIONS}
                onChange={setJpegQuality}
              />
            </div>
          )}
          {qualityMode === "maximum" && (
            <label
              className="screenshot-maximum-size"
              title="JPEG quality is lowered only when needed to meet this limit."
            >
              Maximum file size
              <span>
                <input
                  type="number"
                  min={maximumFileSizeUnit === "kb" ? 10 : maximumFileSizeUnit === "mb" ? 0.01 : 0.00001}
                  step={maximumFileSizeUnit === "kb" ? 1 : maximumFileSizeUnit === "mb" ? 0.01 : 0.00001}
                  value={maximumFileSize}
                  aria-label="Maximum file size"
                  onChange={(event) => setMaximumFileSize(event.target.value)}
                />
                <select
                  aria-label="Screenshot file size unit"
                  value={maximumFileSizeUnit}
                  onChange={(event) => {
                    const nextUnit = event.target.value as ScreenshotFileSizeUnit;
                    const bytes = Number(maximumFileSize)
                      * SCREENSHOT_FILE_SIZE_UNIT_BYTES[maximumFileSizeUnit];
                    setMaximumFileSizeUnit(nextUnit);
                    if (Number.isFinite(bytes)) {
                      setMaximumFileSize(formatScreenshotMaximumFileSizeInput(bytes, nextUnit));
                    }
                  }}
                >
                  <option value="kb">KB</option>
                  <option value="mb">MB</option>
                  <option value="gb">GB</option>
                </select>
              </span>
            </label>
          )}
          <div className="screenshot-export-control screenshot-output-estimate-control" aria-live="polite">
            <span>Est. size</span>
            <strong
              className="screenshot-output-estimate"
              data-pending={estimatePending ? "true" : undefined}
              title="Estimated export file size for the current format, quality, and output size"
            >
              {estimatedSizeLabel}
            </strong>
          </div>
        </div>
        <div className="screenshot-save-row">
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
              <strong>.{screenshotFormatExtension(effectiveExportFormat, artifact.path)}</strong>
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
                  : qualityMode === "preserve" && effectiveExportFormat !== "jpeg"
                    ? savingCopy
                      ? "Lossless export keeps every pixel and saves a new file."
                      : "Lossless export keeps every pixel and replaces the original."
                    : qualityMode === "maximum"
                      ? savingCopy
                        ? "The JPEG stays within the selected limit and saves as a new file."
                        : "The JPEG stays within the selected limit and replaces the original."
                      : qualityMode === "compress"
                        ? savingCopy
                          ? "Compressed JPEG saves as a new file and leaves the original untouched."
                          : "Compressed JPEG replaces the original; turn on Make a copy to keep it."
                        : savingCopy
                          ? "Save creates a new file and leaves the original untouched."
                          : "Save replaces the original; turn on Make a copy to keep it."}
              </div>
            )}
          </div>
          <div className="screenshot-export-actions">
            {!formatRequiresCopy && (
              <label
                className="screenshot-make-copy"
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

function ColorField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <fieldset className="screenshot-color-field">
      <legend>{label}</legend>
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
  if (element.shape === "curved_arrow") return "Curved arrow";
  return element.shape[0].toUpperCase() + element.shape.slice(1);
}

function imageDropLabel(edge: ImageSnapEdge): string {
  if (edge === "top") return "Place above layer";
  if (edge === "right") return "Place to the right";
  if (edge === "left") return "Place to the left";
  return "Place below layer";
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

function layerIconName(element: ScreenshotElement): string {
  if (element.kind === "text") return "text";
  if (element.kind === "path") return "pen";
  return element.kind === "shape" ? element.shape : "image";
}

function toolLabel(tool: ScreenshotTool): string {
  return TOOL_ITEMS.find((item) => item.tool === tool)?.label ?? "Properties";
}

function EditorIcon({ name }: { name: string }) {
  if (name === "select") return <svg viewBox="0 0 24 24"><path d="m5 3 13 9-7 2-3 7Z" /></svg>;
  if (name === "crop") return <svg viewBox="0 0 24 24"><path d="M7 3v14a2 2 0 0 0 2 2h12M3 7h14a2 2 0 0 1 2 2v12" /></svg>;
  if (name === "text") return <svg viewBox="0 0 24 24"><path d="M5 5h14M12 5v14M8 19h8" /></svg>;
  if (name === "rectangle") return <svg viewBox="0 0 24 24"><rect x="4" y="5" width="16" height="14" rx="2" /></svg>;
  if (name === "ellipse") return <svg viewBox="0 0 24 24"><ellipse cx="12" cy="12" rx="8" ry="6.5" /></svg>;
  if (name === "line") return <svg viewBox="0 0 24 24"><path d="M5 19 19 5" /></svg>;
  if (name === "arrow") return <svg viewBox="0 0 24 24"><path d="M4 20 20 4M12 4h8v8" /></svg>;
  if (name === "curved_arrow") return <svg viewBox="0 0 24 24"><path d="M4 18C7 7 14 5 20 8M15 3l5 5-6 3" /></svg>;
  if (name === "pen") return <svg viewBox="0 0 24 24"><path d="M4 16c4-7 6-8 8-3s4 4 8-4M4 20h16" /></svg>;
  if (name === "undo") return <svg viewBox="0 0 24 24"><path d="m9 7-5 5 5 5M5 12h8a6 6 0 0 1 6 6" /></svg>;
  if (name === "redo") return <svg viewBox="0 0 24 24"><path d="m15 7 5 5-5 5M19 12h-8a6 6 0 0 0-6 6" /></svg>;
  if (name === "image") return <svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2" /><circle cx="8" cy="9" r="1.5" /><path d="m5 18 5-5 3 3 2-2 4 4" /></svg>;
  if (name === "trash") return <svg viewBox="0 0 24 24"><path d="M4 7h16M9 7V4h6v3M7 7l1 13h8l1-13M10 11v5M14 11v5" /></svg>;
  if (name === "copy") return <svg viewBox="0 0 24 24"><rect x="8" y="8" width="11" height="11" rx="2" /><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3" /></svg>;
  if (name === "check") return <svg viewBox="0 0 24 24"><path d="m5 12 4.5 4.5L19 7" /></svg>;
  if (name === "save") return <svg viewBox="0 0 24 24"><path d="M5 3h12l2 2v16H5Z M8 3v6h8V3M8 17h8" /></svg>;
  if (name === "plus") return <svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14" /></svg>;
  if (name === "lock") return <svg viewBox="0 0 24 24"><rect x="5" y="10" width="14" height="11" rx="2" /><path d="M8 10V7a4 4 0 0 1 8 0v3" /></svg>;
  if (name === "unlock") return <svg viewBox="0 0 24 24"><rect x="5" y="10" width="14" height="11" rx="2" /><path d="M9 10V7a4 4 0 0 1 7.5-2" /></svg>;
  if (name === "eye") return <svg viewBox="0 0 24 24"><path d="M3 12s3.5-6 9-6 9 6 9 6-3.5 6-9 6-9-6-9-6Z" /><circle cx="12" cy="12" r="2.5" /></svg>;
  if (name === "eye-off") return <svg viewBox="0 0 24 24"><path d="m4 4 16 16M9.5 6.4A9 9 0 0 1 12 6c5.5 0 9 6 9 6a15 15 0 0 1-2.2 2.9M14.4 17.6A9 9 0 0 1 12 18c-5.5 0-9-6-9-6a15 15 0 0 1 2.1-2.8" /></svg>;
  if (name === "duplicate") return <svg viewBox="0 0 24 24"><rect x="8" y="8" width="11" height="11" rx="2" /><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3M13.5 11v5M11 13.5h5" /></svg>;
  if (name === "grip") return <svg viewBox="0 0 24 24"><circle cx="9" cy="7" r=".8" /><circle cx="15" cy="7" r=".8" /><circle cx="9" cy="12" r=".8" /><circle cx="15" cy="12" r=".8" /><circle cx="9" cy="17" r=".8" /><circle cx="15" cy="17" r=".8" /></svg>;
  if (name === "align-center") return <svg viewBox="0 0 24 24"><path d="M5 6h14M8 10h8M5 14h14M8 18h8" /></svg>;
  if (name === "align-right") return <svg viewBox="0 0 24 24"><path d="M5 6h14M9 10h10M5 14h14M9 18h10" /></svg>;
  return <svg viewBox="0 0 24 24"><path d="M5 6h14M5 10h10M5 14h14M5 18h10" /></svg>;
}
