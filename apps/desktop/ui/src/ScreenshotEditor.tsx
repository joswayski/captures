import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
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
  boundedCropRect,
  createScreenshotDocument,
  cropDocument,
  elementBounds,
  estimateCanvasExportBytes,
  expandDocumentForElement,
  hitTestElement,
  imageSizeAtWidth,
  isSupportedImageFile,
  loadImageFile,
  outputDimensions,
  positionImportedImage,
  reorderScreenshotLayers,
  resizeDocumentCanvas,
  translateElement,
  type EditorImageElement,
  type LayerDropPlacement,
  type EditorPoint,
  type EditorRect,
  type ElementStyle,
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
type ScreenshotFileSizeUnit = "kb" | "mb";

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
};

function isFileTransfer(dataTransfer: DataTransfer): boolean {
  return Array.from(dataTransfer.types).includes("Files")
    || dataTransfer.files.length > 0;
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
  context.fillStyle = document.background;
  context.fillRect(0, 0, document.width, document.height);
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = "high";
  for (const element of document.elements) {
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
  }
}

function drawEditorOverlays(
  context: CanvasRenderingContext2D,
  document: ScreenshotDocument,
  selected: ScreenshotElement | null,
  crop: EditorRect | null,
  displayScale: number,
  accentColor: string,
): void {
  const unit = 1 / Math.max(0.01, displayScale);
  if (selected) {
    const bounds = elementBounds(selected);
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

function exportFilter(format: ExportFormat): { name: string; extensions: string[] } {
  if (format === "jpeg") return { name: "JPEG image", extensions: ["jpg", "jpeg"] };
  if (format === "webp") return { name: "WebP image", extensions: ["webp"] };
  return { name: "PNG image", extensions: ["png"] };
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
  const [draggedLayerId, setDraggedLayerId] = useState<string | null>(null);
  const [layerDropTarget, setLayerDropTarget] = useState<LayerDropTarget | null>(null);
  const [exportFormat, setExportFormat] = useState<ExportFormat>("png");
  const [exportSize, setExportSize] = useState<ExportSize>("original");
  const [customExportWidth, setCustomExportWidth] = useState(1_920);
  const [jpegQuality, setJpegQuality] = useState<ScreenshotQuality>("100");
  const [maximumFileSize, setMaximumFileSize] = useState("");
  const [maximumFileSizeUnit, setMaximumFileSizeUnit] =
    useState<ScreenshotFileSizeUnit>("mb");
  const [estimatedBytes, setEstimatedBytes] = useState<number | null>(null);
  const [estimatePending, setEstimatePending] = useState(false);
  const [busy, setBusy] = useState<"copying" | "saving" | null>(null);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [saved, setSaved] = useState<SavedScreenshotEdit | null>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const imageCacheRef = useRef(new Map<string, CachedImage>());
  const objectUrlsRef = useRef(new Set<string>());
  const gestureRef = useRef<EditorGesture | null>(null);
  const dropDepthRef = useRef(0);

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

  const commitDocument = useCallback((next: ScreenshotDocument) => {
    const current = documentRef.current;
    if (!current || JSON.stringify(current) === JSON.stringify(next)) return;
    setUndoStack((stack) => [...stack.slice(-99), current]);
    setRedoStack([]);
    replaceDocument(next);
    setSaved(null);
    setStatus("");
  }, [replaceDocument]);

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
        if (payload === artifactId) setError("The original screenshot is no longer available.");
      });
      const loaded = await invoke<CaptureArtifact | null>("get_artifact", { artifactId });
      if (!active) return;
      if (!loaded) throw new Error("The screenshot is no longer available.");
      const next = createScreenshotDocument(
        loaded.full_url,
        loaded.width,
        loaded.height,
      );
      ensureImage(loaded.full_url);
      setArtifact(loaded);
      replaceDocument(next);
      setCustomExportWidth(loaded.width);
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
  }, [artifactId, ensureImage, replaceDocument]);

  const selected = useMemo(() => (
    editorDocument?.elements.find((element) => element.id === selectedId) ?? null
  ), [editorDocument, selectedId]);

  const displayScale = zoomMode === "fit" ? fitScale : zoom / 100;

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
    );
  }, [
    cropSelection,
    displayScale,
    editorDocument,
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
    if (!current || !element || (element.kind === "image" && element.source === "background")) return;
    commitDocument({
      ...current,
      elements: current.elements.filter(({ id }) => id !== selectedId),
    });
    setSelectedId(null);
  }, [commitDocument, selectedId]);

  const nudgeSelected = useCallback((deltaX: number, deltaY: number) => {
    const current = documentRef.current;
    const element = current?.elements.find(({ id }) => id === selectedId);
    if (!current || !element || (element.kind === "image" && element.source === "background")) return;
    commitDocument(replaceElement(
      current,
      element.id,
      translateElement(element, deltaX, deltaY),
    ));
  }, [commitDocument, selectedId]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.matches("input, textarea, select, [contenteditable=true]")) return;
      const command = event.metaKey || event.ctrlKey;
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
  }, [deleteSelected, nudgeSelected, redo, undo]);

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
    setStatus("");
    setSaved(null);

    if (tool === "select") {
      const element = hitTestElement(current.elements, point, 10 / displayScale);
      setSelectedId(element?.id ?? null);
      if (element) {
        gestureRef.current = {
          kind: "move",
          pointerId: event.pointerId,
          origin: point,
          element,
          initialDocument: current,
        };
        event.currentTarget.setPointerCapture(event.pointerId);
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
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    const point = canvasPoint(event);
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
      const moved = translateElement(
        gesture.element,
        point.x - gesture.origin.x,
        point.y - gesture.origin.y,
      );
      replaceDocument(replaceElement(
        gesture.initialDocument,
        gesture.element.id,
        moved,
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
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (gesture.kind === "crop") return;
    const current = documentRef.current;
    if (!current || JSON.stringify(current) === JSON.stringify(gesture.initialDocument)) return;
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

  const moveLayer = (direction: "front" | "back") => {
    const current = documentRef.current;
    if (!current || !selectedId) return;
    const index = current.elements.findIndex(({ id }) => id === selectedId);
    if (index < 0) return;
    const minimum = current.elements[0]?.kind === "image"
      && current.elements[0].source === "background" ? 1 : 0;
    const destination = direction === "front"
      ? current.elements.length - 1
      : minimum;
    if (index === destination) return;
    const elements = [...current.elements];
    const [element] = elements.splice(index, 1);
    elements.splice(destination, 0, element);
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

  const loadDroppedFiles = async (files: File[], point?: EditorPoint) => {
    const images = files.filter(isSupportedImageFile);
    if (images.length === 0) {
      setError("Drop PNG, JPEG, WebP, GIF, or another image file.");
      return;
    }
    const initial = documentRef.current;
    if (!initial) return;
    let next = initial;
    let lastId: string | null = null;
    const createdUrls: string[] = [];
    try {
      for (const [index, file] of images.entries()) {
        // Prefer a blob object URL (cheap, revocable). Fall back to a data URL
        // if the webview rejects the blob load — historically our CSP omitted
        // blob: from img-src, which produced "could not be loaded" on drop.
        const image = await loadImageFile(file);
        createdUrls.push(image.src);
        if (image.src.startsWith("blob:")) {
          objectUrlsRef.current.add(image.src);
        }
        imageCacheRef.current.set(image.src, { image, status: "loaded" });
        const position = positionImportedImage(
          image.naturalWidth,
          image.naturalHeight,
          next,
          point
            ? {
              x: point.x + index * 24,
              y: point.y + index * 24,
            }
            : undefined,
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
        };
        next = expandDocumentForElement(next, element);
        lastId = element.id;
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

  const renderFlattened = useCallback((): HTMLCanvasElement => {
    const current = documentRef.current;
    if (!current) throw new Error("The editor is still loading.");
    const missing = current.elements
      .filter((element): element is EditorImageElement => element.kind === "image")
      .find((element) => imageCacheRef.current.get(element.src)?.status !== "loaded");
    if (missing) throw new Error(`${missing.name} has not finished loading.`);
    const source = window.document.createElement("canvas");
    source.width = current.width;
    source.height = current.height;
    const sourceContext = source.getContext("2d");
    if (!sourceContext) throw new Error("Canvas rendering is unavailable.");
    renderScreenshot(sourceContext, current, imageCacheRef.current);
    const requestedWidth = exportSize === "original"
      ? current.width
      : exportSize === "custom"
        ? customExportWidth
        : Math.round(current.width * Number(exportSize) / 100);
    const dimensions = outputDimensions(current.width, current.height, requestedWidth);
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
  }, [customExportWidth, exportSize]);

  useEffect(() => {
    if (!editorDocument) return;
    let cancelled = false;
    const timer = window.setTimeout(() => {
      if (cancelled) return;
      setEstimatePending(true);
      void (async () => {
        try {
          const canvas = renderFlattened();
          const bytes = await estimateCanvasExportBytes(
            canvas,
            exportFormat,
            Number(jpegQuality),
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
    customExportWidth,
    editorDocument,
    exportFormat,
    exportSize,
    imageRevision,
    jpegQuality,
    renderFlattened,
  ]);

  const copyEditedImage = async () => {
    if (busy) return;
    setBusy("copying");
    setError("");
    setStatus("");
    try {
      const imagePng = await canvasPngBytes(renderFlattened());
      await invoke("copy_screenshot_edit", { imagePng });
      setStatus("Edited screenshot copied to the clipboard.");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(null);
    }
  };

  const saveEditedImage = async () => {
    if (!artifact || busy) return;
    const maximumSizeText = maximumFileSize.trim();
    const maximumSizeBytes = maximumSizeText
      ? Math.floor(
        Number(maximumSizeText) * SCREENSHOT_FILE_SIZE_UNIT_BYTES[maximumFileSizeUnit],
      )
      : null;
    if (
      maximumSizeText
      && (!Number.isFinite(maximumSizeBytes) || maximumSizeBytes === null || maximumSizeBytes < 10_000)
    ) {
      setError("Enter a maximum file size of at least 10 KB, or leave it blank.");
      return;
    }
    setBusy("saving");
    setError("");
    setStatus("");
    try {
      const defaultPath = await invoke<string>("default_screenshot_edit_path", {
        artifactId: artifact.id,
        format: exportFormat,
      });
      const destinationPath = await saveDialog({
        defaultPath,
        filters: [exportFilter(exportFormat)],
      });
      if (!destinationPath) return;
      const imagePng = await canvasPngBytes(renderFlattened());
      const result = await invoke<SavedScreenshotEdit>("save_screenshot_edit", {
        request: {
          artifact_id: artifact.id,
          destination_path: destinationPath,
          format: exportFormat,
          jpeg_quality: Number(jpegQuality),
          max_size_bytes: maximumSizeBytes,
          image_png: imagePng,
        },
      });
      setSaved(result);
      setStatus(`Saved ${result.path}`);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(null);
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

  const output = outputDimensions(
    editorDocument.width,
    editorDocument.height,
    exportSize === "original"
      ? editorDocument.width
      : exportSize === "custom"
        ? customExportWidth
        : Math.round(editorDocument.width * Number(exportSize) / 100),
  );

  return (
    <main
      className={`screenshot-editor${dragActive ? " screenshot-editor-drag-active" : ""}`}
      onDragEnter={(event) => {
        if (!isFileTransfer(event.dataTransfer)) return;
        event.preventDefault();
        dropDepthRef.current += 1;
        setDragActive(true);
      }}
      onDragOver={(event) => {
        if (!isFileTransfer(event.dataTransfer)) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "copy";
      }}
      onDragLeave={(event) => {
        if (!dragActive) return;
        event.preventDefault();
        dropDepthRef.current = Math.max(0, dropDepthRef.current - 1);
        if (dropDepthRef.current === 0) setDragActive(false);
      }}
      onDrop={(event) => {
        if (!isFileTransfer(event.dataTransfer)) return;
        event.preventDefault();
        dropDepthRef.current = 0;
        setDragActive(false);
        const canvas = canvasRef.current;
        let point: EditorPoint | undefined;
        if (canvas) {
          const bounds = canvas.getBoundingClientRect();
          if (
            event.clientX >= bounds.left
            && event.clientX <= bounds.right
            && event.clientY >= bounds.top
            && event.clientY <= bounds.bottom
          ) {
            point = {
              x: (event.clientX - bounds.left) * canvas.width / Math.max(1, bounds.width),
              y: (event.clientY - bounds.top) * canvas.height / Math.max(1, bounds.height),
            };
          }
        }
        void loadDroppedFiles(Array.from(event.dataTransfer.files), point);
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
              onClick={() => setZoomMode("fit")}
            >
              Fit
            </button>
            <select
              aria-label="Canvas zoom"
              value={zoomMode === "fit" ? "fit" : String(zoom)}
              onChange={(event) => {
                if (event.target.value === "fit") setZoomMode("fit");
                else {
                  setZoomMode("manual");
                  setZoom(Number(event.target.value));
                }
              }}
            >
              <option value="fit">Fit</option>
              <option value="50">50%</option>
              <option value="100">100%</option>
              <option value="200">200%</option>
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
          className="screenshot-canvas-surface"
          style={{
            width: editorDocument.width * displayScale,
            height: editorDocument.height * displayScale,
          }}
        >
          <canvas
            ref={canvasRef}
            width={editorDocument.width}
            height={editorDocument.height}
            style={{
              width: editorDocument.width * displayScale,
              height: editorDocument.height * displayScale,
            }}
            className={`screenshot-canvas tool-${tool}`}
            onPointerDown={startPointer}
            onPointerMove={movePointer}
            onPointerUp={finishPointer}
            onPointerCancel={finishPointer}
          />
        </div>
        {dragActive && (
          <div className="screenshot-drop-overlay" aria-hidden="true">
            <EditorIcon name="image" />
            <strong>Drop images to combine them</strong>
            <span>Each image stays movable and resizable.</span>
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
              const locked = element.kind === "image" && element.source === "background";
              const dropPlacement = layerDropTarget?.id === element.id
                ? layerDropTarget.placement
                : null;
              return (
                <li
                  key={element.id}
                  className={[
                    selectedId === element.id ? "active" : "",
                    locked ? "locked" : "",
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
                    const placement = locked || event.clientY < bounds.top + bounds.height / 2
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
                    aria-pressed={selectedId === element.id}
                    onClick={() => {
                      setTool("select");
                      setCropSelection(null);
                      setSelectedId(locked ? null : element.id);
                    }}
                  >
                    <span className="screenshot-layer-grip" aria-hidden="true">
                      <EditorIcon name={locked ? "lock" : "grip"} />
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
                </li>
              );
            })}
          </ol>
        </section>

        <section className="screenshot-properties" aria-label="Tool properties">
        <div className="screenshot-properties-heading">
          <strong>{selected ? elementLabel(selected) : toolLabel(tool)}</strong>
          {selected && !(selected.kind === "image" && selected.source === "background") && (
            <button type="button" aria-label="Delete selected item" onClick={deleteSelected}>
              <EditorIcon name="trash" />
            </button>
          )}
        </div>

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

        {selected?.kind === "image" && selected.source === "imported" && (
          <section className="screenshot-property-section">
            <p className="screenshot-file-name" title={selected.name}>{selected.name}</p>
            <div className="screenshot-number-pair">
              <label>
                Width
                <input
                  type="number"
                  min={1}
                  max={16_384}
                  value={Math.round(selected.width)}
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

        {selected && !(selected.kind === "image" && selected.source === "background") && (
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
          <ColorField
            label="Canvas background"
            value={editorDocument.background}
            onChange={(background) => commitDocument({ ...editorDocument, background })}
          />
        </section>
        </section>
      </aside>

      <footer className="screenshot-export-bar">
        <div className="screenshot-export-settings">
          <label>
            Format
            <select value={exportFormat} onChange={(event) => setExportFormat(event.target.value as ExportFormat)}>
              <option value="png">PNG · lossless</option>
              <option value="jpeg">JPEG</option>
              <option value="webp">WebP · lossless</option>
            </select>
          </label>
          <label>
            Output size
            <select value={exportSize} onChange={(event) => setExportSize(event.target.value as ExportSize)}>
              <option value="original">Original</option>
              <option value="75">75%</option>
              <option value="50">50%</option>
              <option value="custom">Custom width</option>
            </select>
          </label>
          {exportSize === "custom" && (
            <label>
              Width
              <input
                type="number"
                min={1}
                max={16_384}
                value={customExportWidth}
                onChange={(event) => setCustomExportWidth(Number(event.target.value))}
              />
            </label>
          )}
          <div className="screenshot-export-control screenshot-quality">
            <span>Quality</span>
            {exportFormat === "jpeg" ? (
              <NotchedSlider
                ariaLabel="Image quality"
                value={jpegQuality}
                options={SCREENSHOT_QUALITY_OPTIONS}
                onChange={setJpegQuality}
              />
            ) : (
              <strong>Maximum · lossless</strong>
            )}
          </div>
          <label className="screenshot-maximum-size">
            Maximum size
            <span>
              <input
                type="number"
                min={maximumFileSizeUnit === "kb" ? 10 : 0.01}
                step={maximumFileSizeUnit === "kb" ? 1 : 0.01}
                value={maximumFileSize}
                placeholder="No limit"
                aria-label="Maximum file size"
                onChange={(event) => setMaximumFileSize(event.target.value)}
              />
              <select
                aria-label="Screenshot file size unit"
                value={maximumFileSizeUnit}
                onChange={(event) => setMaximumFileSizeUnit(
                  event.target.value as ScreenshotFileSizeUnit,
                )}
              >
                <option value="kb">KB</option>
                <option value="mb">MB</option>
              </select>
            </span>
          </label>
          <div className="screenshot-output-meta" aria-live="polite">
            <span className="screenshot-output-dimensions">{output.width} × {output.height}</span>
            <span
              className="screenshot-output-estimate"
              data-pending={estimatePending ? "true" : undefined}
              title="Estimated export file size for the current format, quality, and output size"
            >
              {estimatePending && estimatedBytes === null
                ? "Estimating…"
                : estimatedBytes === null
                  ? "Size unavailable"
                  : `≈ ${formatFileSize(estimatedBytes)}`}
            </span>
          </div>
        </div>
        <div className={`screenshot-export-status${error ? " error" : ""}`} role={error ? "alert" : "status"}>
          {error || status || "Saving creates a new copy and preserves the original."}
        </div>
        <div className="screenshot-export-actions">
          {saved && <button type="button" onClick={() => void showSavedFile()}>Show in folder</button>}
          <button type="button" disabled={busy !== null} onClick={() => void copyEditedImage()}>
            <EditorIcon name="copy" />{busy === "copying" ? "Copying…" : "Copy"}
          </button>
          <button
            type="button"
            className="primary"
            disabled={busy !== null}
            onClick={() => void saveEditedImage()}
          >
            <EditorIcon name="save" />{busy === "saving" ? "Saving…" : "Save copy"}
          </button>
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
    return element.source === "background" ? "Original screenshot" : element.name;
  }
  if (element.kind === "text") return "Text";
  if (element.kind === "path") return "Freehand drawing";
  if (element.shape === "curved_arrow") return "Curved arrow";
  return element.shape[0].toUpperCase() + element.shape.slice(1);
}

function elementLayerName(element: ScreenshotElement): string {
  if (element.kind === "text") {
    return element.text.trim().split("\n")[0]?.slice(0, 42) || "Text";
  }
  return elementLabel(element);
}

function elementKindLabel(element: ScreenshotElement): string {
  if (element.kind === "image") {
    return element.source === "background" ? "Locked background" : "Image";
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
  if (name === "save") return <svg viewBox="0 0 24 24"><path d="M5 3h12l2 2v16H5Z M8 3v6h8V3M8 17h8" /></svg>;
  if (name === "plus") return <svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14" /></svg>;
  if (name === "lock") return <svg viewBox="0 0 24 24"><rect x="5" y="10" width="14" height="11" rx="2" /><path d="M8 10V7a4 4 0 0 1 8 0v3" /></svg>;
  if (name === "grip") return <svg viewBox="0 0 24 24"><circle cx="9" cy="7" r=".8" /><circle cx="15" cy="7" r=".8" /><circle cx="9" cy="12" r=".8" /><circle cx="15" cy="12" r=".8" /><circle cx="9" cy="17" r=".8" /><circle cx="15" cy="17" r=".8" /></svg>;
  if (name === "align-center") return <svg viewBox="0 0 24 24"><path d="M5 6h14M8 10h8M5 14h14M8 18h8" /></svg>;
  if (name === "align-right") return <svg viewBox="0 0 24 24"><path d="M5 6h14M9 10h10M5 14h14M9 18h10" /></svg>;
  return <svg viewBox="0 0 24 24"><path d="M5 6h14M5 10h10M5 14h14M5 18h10" /></svg>;
}
