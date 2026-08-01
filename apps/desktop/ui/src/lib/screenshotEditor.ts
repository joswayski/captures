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
  | "curved_arrow"
  | "pen";

export type ShapeKind = "rectangle" | "ellipse" | "line" | "arrow" | "curved_arrow";

export type ElementStyle = {
  color: string;
  fill: string | null;
  strokeWidth: number;
};

type EditorElementBase = {
  id: string;
  x: number;
  y: number;
};

export type EditorImageElement = EditorElementBase & {
  kind: "image";
  source: "background" | "imported";
  src: string;
  name: string;
  width: number;
  height: number;
  naturalWidth: number;
  naturalHeight: number;
};

export type EditorTextElement = EditorElementBase & {
  kind: "text";
  text: string;
  fontSize: number;
  fontFamily: "sans" | "serif" | "mono";
  bold: boolean;
  italic: boolean;
  align: "left" | "center" | "right";
  color: string;
  background: string | null;
};

export type EditorShapeElement = EditorElementBase & {
  kind: "shape";
  shape: ShapeKind;
  endX: number;
  endY: number;
  bend: number;
  style: ElementStyle;
};

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
  background: string;
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
      x: 0,
      y: 0,
      width,
      height,
      naturalWidth: width,
      naturalHeight: height,
    }],
  };
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

export function expandDocumentForElement(
  document: ScreenshotDocument,
  element: ScreenshotElement,
  padding = 24,
): ScreenshotDocument {
  const bounds = elementBounds(element);
  return {
    ...document,
    width: Math.max(document.width, Math.ceil(bounds.x + bounds.width + padding)),
    height: Math.max(document.height, Math.ceil(bounds.y + bounds.height + padding)),
    elements: [...document.elements, element],
  };
}

export function isSupportedImageFile(file: Pick<File, "name" | "type">): boolean {
  if (file.type.toLowerCase().startsWith("image/")) return true;
  const extension = file.name.split(".").at(-1)?.toLowerCase() ?? "";
  return SUPPORTED_IMAGE_EXTENSIONS.has(extension);
}

/**
 * Screenshot elements are stored back-to-front, while the layer panel is
 * presented front-to-back. Reorder in the panel's visual order, then convert
 * back without ever allowing the original screenshot below another layer.
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
    || (moved.kind === "image" && moved.source === "background")
  ) {
    return elements;
  }

  const backgrounds = elements.filter((element) => (
    element.kind === "image" && element.source === "background"
  ));
  const visualLayers = elements
    .filter((element) => !(element.kind === "image" && element.source === "background"))
    .slice()
    .reverse();
  const movedIndex = visualLayers.findIndex((element) => element.id === movedId);
  if (movedIndex < 0) return elements;
  const [layer] = visualLayers.splice(movedIndex, 1);
  const targetIsBackground = backgrounds.some((element) => element.id === targetId);
  const targetIndex = visualLayers.findIndex((element) => element.id === targetId);
  if (!targetIsBackground && targetIndex < 0) return elements;
  const destination = targetIsBackground
    ? visualLayers.length
    : targetIndex + (placement === "after" ? 1 : 0);
  visualLayers.splice(destination, 0, layer);
  return [...backgrounds, ...visualLayers.reverse()];
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
    };
  }
  return {
    ...element,
    x: element.x + deltaX,
    y: element.y + deltaY,
  };
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
    // Text bounds are derived from font size; scale type size with the box height.
    const scale = Math.max(0.05, scaleY);
    const topLeft = mapPoint({ x: element.x, y: element.y });
    return {
      ...element,
      x: topLeft.x,
      y: topLeft.y,
      fontSize: Math.max(8, Math.round(element.fontSize * scale)),
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
    const lines = element.text.split("\n");
    const width = Math.max(
      element.fontSize,
      ...lines.map((line) => Math.max(1, line.length) * element.fontSize * 0.62),
    );
    return {
      x: element.x,
      y: element.y,
      width,
      height: Math.max(1, lines.length) * element.fontSize * 1.25,
    };
  }
  if (element.kind === "shape") {
    const rect = normalizeRect(
      { x: element.x, y: element.y },
      { x: element.endX, y: element.endY },
    );
    const curvePadding = element.shape === "curved_arrow"
      ? Math.hypot(rect.width, rect.height) * Math.abs(element.bend)
      : 0;
    const strokePadding = Math.max(8, element.style.strokeWidth * 3);
    return {
      x: rect.x - strokePadding - curvePadding,
      y: rect.y - strokePadding - curvePadding,
      width: Math.max(1, rect.width) + (strokePadding + curvePadding) * 2,
      height: Math.max(1, rect.height) + (strokePadding + curvePadding) * 2,
    };
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
    if (element.kind === "image" && element.source === "background") continue;
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
