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
