/**
 * Pixel tools for removing (and restoring) background on image layers.
 * Operates on natural-resolution ImageData; the editor bakes results to PNG.
 */

import {
  imageOrientationMatrix,
  imageOrientationSwapsAxes,
  imageSourceDisplaySize,
  type EditorImageElement,
  type EditorPoint,
  type ScreenshotElement,
} from "./screenshotEditor";

export type Rgba = {
  r: number;
  g: number;
  b: number;
  a: number;
};

export type RemoveBackgroundMode = "wand" | "erase" | "restore";

/** Default magic-wand tolerance (0–255 max channel delta). */
export const DEFAULT_WAND_TOLERANCE = 36;

/** Default brush radius in document (layer) pixels before natural scaling. */
export const DEFAULT_REMOVE_BG_BRUSH_SIZE = 28;

/** How hard the erase/restore brush edge is (1 = hard circle). */
export const DEFAULT_BRUSH_HARDNESS = 0.82;

/**
 * Topmost visible image layer under a document-space point.
 * Includes locked layers so the original capture can be edited without unlocking.
 */
export function hitTestImageElement(
  elements: readonly ScreenshotElement[],
  point: EditorPoint,
): EditorImageElement | null {
  for (let index = elements.length - 1; index >= 0; index -= 1) {
    const element = elements[index];
    if (!element.visible || element.kind !== "image") continue;
    if (
      point.x >= element.x
      && point.x < element.x + element.width
      && point.y >= element.y
      && point.y < element.y + element.height
    ) {
      return element;
    }
  }
  return null;
}

/**
 * Map a document-space point to integer natural-image pixel coordinates.
 * Returns null when the point is outside the layer bounds.
 */
export function documentPointToImagePixel(
  element: EditorImageElement,
  point: EditorPoint,
): { x: number; y: number } | null {
  if (element.width <= 0 || element.height <= 0) return null;
  const localX = point.x - element.x;
  const localY = point.y - element.y;
  if (
    localX < 0
    || localY < 0
    || localX >= element.width
    || localY >= element.height
  ) {
    return null;
  }
  const matrix = imageOrientationMatrix(element.orientation);
  const displayedSource = imageSourceDisplaySize(element);
  const displayX = localX - element.width / 2;
  const displayY = localY - element.height / 2;
  // Orientation matrices are orthonormal, so their inverse is the transpose.
  const sourceX = matrix.a * displayX + matrix.b * displayY;
  const sourceY = matrix.c * displayX + matrix.d * displayY;
  const sourceRatioX = (sourceX + displayedSource.width / 2) / displayedSource.width;
  const sourceRatioY = (sourceY + displayedSource.height / 2) / displayedSource.height;
  const x = Math.min(
    element.naturalWidth - 1,
    Math.max(0, Math.floor(sourceRatioX * element.naturalWidth)),
  );
  const y = Math.min(
    element.naturalHeight - 1,
    Math.max(0, Math.floor(sourceRatioY * element.naturalHeight)),
  );
  return { x, y };
}

/** Document-space brush radius → natural-image pixel radius. */
export function brushRadiusInNaturalPixels(
  element: EditorImageElement,
  documentBrushSize: number,
): number {
  const displayedNaturalWidth = imageOrientationSwapsAxes(element.orientation)
    ? element.naturalHeight
    : element.naturalWidth;
  const scale = displayedNaturalWidth / Math.max(1, element.width);
  return Math.max(1, documentBrushSize * scale * 0.5);
}

/**
 * On-screen diameter of the remove-bg erase/restore brush.
 * `documentBrushSize` is a diameter in document pixels; multiply by zoom.
 */
export function removeBgBrushScreenDiameter(
  documentBrushSize: number,
  displayScale: number,
): number {
  return Math.max(1, documentBrushSize * Math.max(0.01, displayScale));
}

export function samplePixel(data: ImageData, x: number, y: number): Rgba | null {
  if (x < 0 || y < 0 || x >= data.width || y >= data.height) return null;
  const index = (y * data.width + x) * 4;
  return {
    r: data.data[index],
    g: data.data[index + 1],
    b: data.data[index + 2],
    a: data.data[index + 3],
  };
}

/**
 * Sample one natural-image pixel without decoding the full frame.
 * Pass a reusable 1×1 canvas via `scratch` to avoid allocating on every hover.
 */
export function sampleImagePixel(
  image: CanvasImageSource & {
    naturalWidth?: number;
    naturalHeight?: number;
    width: number;
    height: number;
  },
  x: number,
  y: number,
  scratch?: HTMLCanvasElement,
): Rgba | null {
  const width = Math.max(
    1,
    Math.round(image.naturalWidth ?? (image as HTMLImageElement).width),
  );
  const height = Math.max(
    1,
    Math.round(image.naturalHeight ?? (image as HTMLImageElement).height),
  );
  if (x < 0 || y < 0 || x >= width || y >= height) return null;

  const canvas = scratch ?? document.createElement("canvas");
  if (canvas.width !== 1) canvas.width = 1;
  if (canvas.height !== 1) canvas.height = 1;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) return null;
  try {
    context.clearRect(0, 0, 1, 1);
    context.drawImage(image, x, y, 1, 1, 0, 0, 1, 1);
    const data = context.getImageData(0, 0, 1, 1).data;
    return {
      r: data[0],
      g: data[1],
      b: data[2],
      a: data[3],
    };
  } catch {
    // Tainted canvas (CORS) or incomplete image — loupe still works without hex.
    return null;
  }
}

/** CSS color for loupe fill / swatches (`rgba` so transparency is visible). */
export function rgbaToCss(color: Rgba): string {
  const alpha = Math.round((color.a / 255) * 1000) / 1000;
  return `rgba(${color.r}, ${color.g}, ${color.b}, ${alpha})`;
}

/** `#rrggbb` (ignores alpha) for compact loupe labels. */
export function rgbaToHex(color: Rgba): string {
  const channel = (value: number) => value.toString(16).padStart(2, "0");
  return `#${channel(color.r)}${channel(color.g)}${channel(color.b)}`;
}

/**
 * On-screen loupe for the remove-bg wand: odd natural-pixel neighborhood
 * drawn nearest-neighbor into a circle so the sampled color is readable.
 */
export const WAND_LOUPE_SAMPLE_EXTENT = 11;
/** CSS diameter of the wand color loupe circle. */
export const WAND_LOUPE_SIZE_PX = 84;
/** Offset from the crosshair so the loupe does not cover the sample pixel. */
export const WAND_LOUPE_OFFSET_PX = 18;

/**
 * Paint a nearest-neighbor zoom of the pixels around `(pixelX, pixelY)` into
 * `target`. Returns false when the source image cannot be drawn.
 */
export function paintWandColorLoupe(
  target: HTMLCanvasElement,
  image: CanvasImageSource & {
    naturalWidth?: number;
    naturalHeight?: number;
    width: number;
    height: number;
  },
  pixelX: number,
  pixelY: number,
  options?: {
    sizePx?: number;
    sampleExtent?: number;
    devicePixelRatio?: number;
  },
): boolean {
  const sizePx = options?.sizePx ?? WAND_LOUPE_SIZE_PX;
  const sampleExtent = options?.sampleExtent ?? WAND_LOUPE_SAMPLE_EXTENT;
  const dpr = Math.max(1, options?.devicePixelRatio ?? 1);
  const oddExtent = Math.max(1, sampleExtent | 1); // force odd so a true center pixel exists
  const half = Math.floor(oddExtent / 2);
  const width = Math.max(
    1,
    Math.round(image.naturalWidth ?? (image as HTMLImageElement).width),
  );
  const height = Math.max(
    1,
    Math.round(image.naturalHeight ?? (image as HTMLImageElement).height),
  );

  const pixelSize = Math.max(1, Math.round(sizePx * dpr));
  if (target.width !== pixelSize) target.width = pixelSize;
  if (target.height !== pixelSize) target.height = pixelSize;
  const context = target.getContext("2d");
  if (!context) return false;

  context.setTransform(dpr, 0, 0, dpr, 0, 0);
  context.clearRect(0, 0, sizePx, sizePx);

  // Checkerboard so fully transparent samples read as “empty” rather than black.
  const check = 6;
  for (let row = 0; row < sizePx; row += check) {
    for (let col = 0; col < sizePx; col += check) {
      const dark = ((row / check) + (col / check)) % 2 === 0;
      context.fillStyle = dark ? "#c4c4c8" : "#ececee";
      context.fillRect(col, row, check, check);
    }
  }

  // Clamp the sample window so edge pixels still fill the loupe.
  const srcX = Math.min(width - 1, Math.max(0, pixelX)) - half;
  const srcY = Math.min(height - 1, Math.max(0, pixelY)) - half;

  context.imageSmoothingEnabled = false;
  try {
    context.drawImage(
      image,
      srcX,
      srcY,
      oddExtent,
      oddExtent,
      0,
      0,
      sizePx,
      sizePx,
    );
  } catch {
    return false;
  }

  // Subtle grid so individual source pixels read as tiles.
  const cell = sizePx / oddExtent;
  context.strokeStyle = "rgba(0, 0, 0, 0.18)";
  context.lineWidth = 1 / dpr;
  context.beginPath();
  for (let i = 1; i < oddExtent; i += 1) {
    const edge = i * cell;
    context.moveTo(edge, 0);
    context.lineTo(edge, sizePx);
    context.moveTo(0, edge);
    context.lineTo(sizePx, edge);
  }
  context.stroke();

  // Highlight the exact center sample (what the wand will key on).
  const center = half * cell;
  context.strokeStyle = "rgba(255, 255, 255, 0.95)";
  context.lineWidth = 1.5;
  context.strokeRect(center + 0.5, center + 0.5, cell - 1, cell - 1);
  context.strokeStyle = "rgba(0, 0, 0, 0.55)";
  context.lineWidth = 1;
  context.strokeRect(center, center, cell, cell);

  return true;
}

/** Place the loupe beside the cursor, flipping near the viewport edges. */
export function wandLoupeScreenPosition(
  clientX: number,
  clientY: number,
  options?: {
    sizePx?: number;
    offsetPx?: number;
    viewportWidth?: number;
    viewportHeight?: number;
  },
): { left: number; top: number } {
  const sizePx = options?.sizePx ?? WAND_LOUPE_SIZE_PX;
  const offsetPx = options?.offsetPx ?? WAND_LOUPE_OFFSET_PX;
  const viewportWidth = options?.viewportWidth
    ?? (typeof window !== "undefined" ? window.innerWidth : clientX + sizePx + offsetPx);
  const viewportHeight = options?.viewportHeight
    ?? (typeof window !== "undefined" ? window.innerHeight : clientY + sizePx + offsetPx);
  const margin = 8;

  let left = clientX + offsetPx;
  let top = clientY + offsetPx;
  if (left + sizePx > viewportWidth - margin) {
    left = clientX - offsetPx - sizePx;
  }
  if (top + sizePx > viewportHeight - margin) {
    top = clientY - offsetPx - sizePx;
  }
  left = Math.max(margin, Math.min(left, viewportWidth - sizePx - margin));
  top = Math.max(margin, Math.min(top, viewportHeight - sizePx - margin));
  return { left, top };
}

/** Chebyshev distance on RGB channels (classic magic-wand style). */
export function colorDistanceRgb(a: Rgba, b: Rgba): number {
  return Math.max(
    Math.abs(a.r - b.r),
    Math.abs(a.g - b.g),
    Math.abs(a.b - b.b),
  );
}

function pixelMatches(
  data: Uint8ClampedArray,
  index: number,
  target: Rgba,
  tolerance: number,
): boolean {
  // Skip already-transparent pixels so wand/erase do not thrash empty regions.
  if (data[index + 3] === 0) return false;
  const sample: Rgba = {
    r: data[index],
    g: data[index + 1],
    b: data[index + 2],
    a: data[index + 3],
  };
  return colorDistanceRgb(sample, target) <= tolerance;
}

function clearPixel(data: Uint8ClampedArray, index: number): void {
  data[index] = 0;
  data[index + 1] = 0;
  data[index + 2] = 0;
  data[index + 3] = 0;
}

/**
 * Flood-fill (contiguous) or global color key to full transparency.
 * Returns the number of pixels cleared.
 */
export function removeColorToTransparent(
  imageData: ImageData,
  startX: number,
  startY: number,
  tolerance: number,
  contiguous = true,
): number {
  const { width, height, data } = imageData;
  if (startX < 0 || startY < 0 || startX >= width || startY >= height) {
    return 0;
  }
  const seedIndex = (startY * width + startX) * 4;
  if (data[seedIndex + 3] === 0) return 0;

  const target: Rgba = {
    r: data[seedIndex],
    g: data[seedIndex + 1],
    b: data[seedIndex + 2],
    a: data[seedIndex + 3],
  };
  const clampedTolerance = Math.max(0, Math.min(255, Math.round(tolerance)));
  let changed = 0;

  if (!contiguous) {
    for (let index = 0; index < data.length; index += 4) {
      if (pixelMatches(data, index, target, clampedTolerance)) {
        clearPixel(data, index);
        changed += 1;
      }
    }
    return changed;
  }

  const visited = new Uint8Array(width * height);
  const queue = new Int32Array(width * height);
  let head = 0;
  let tail = 0;
  queue[tail++] = startY * width + startX;
  visited[startY * width + startX] = 1;

  while (head < tail) {
    const pixel = queue[head++];
    const x = pixel % width;
    const y = (pixel - x) / width;
    const index = pixel * 4;
    if (!pixelMatches(data, index, target, clampedTolerance)) continue;
    clearPixel(data, index);
    changed += 1;

    if (x > 0 && !visited[pixel - 1]) {
      visited[pixel - 1] = 1;
      queue[tail++] = pixel - 1;
    }
    if (x + 1 < width && !visited[pixel + 1]) {
      visited[pixel + 1] = 1;
      queue[tail++] = pixel + 1;
    }
    if (y > 0 && !visited[pixel - width]) {
      visited[pixel - width] = 1;
      queue[tail++] = pixel - width;
    }
    if (y + 1 < height && !visited[pixel + width]) {
      visited[pixel + width] = 1;
      queue[tail++] = pixel + width;
    }
  }

  return changed;
}

/**
 * Circular brush stamp in natural image space.
 * - erase: multiplies alpha toward 0 (soft edge when hardness < 1)
 * - restore: copies RGB+A from `original` (required for restore)
 */
export function stampRemoveBackgroundBrush(
  working: ImageData,
  centerX: number,
  centerY: number,
  radius: number,
  mode: "erase" | "restore",
  original: ImageData | null = null,
  hardness = DEFAULT_BRUSH_HARDNESS,
): number {
  if (radius <= 0) return 0;
  if (mode === "restore" && !original) return 0;
  if (
    mode === "restore"
    && original
    && (original.width !== working.width || original.height !== working.height)
  ) {
    return 0;
  }

  const { width, height, data } = working;
  const hard = Math.max(0, Math.min(1, hardness));
  const r = Math.max(0.5, radius);
  const r2 = r * r;
  const softStart = r * hard;
  const softStart2 = softStart * softStart;
  const minX = Math.max(0, Math.floor(centerX - r));
  const maxX = Math.min(width - 1, Math.ceil(centerX + r));
  const minY = Math.max(0, Math.floor(centerY - r));
  const maxY = Math.min(height - 1, Math.ceil(centerY + r));
  let changed = 0;

  for (let y = minY; y <= maxY; y += 1) {
    for (let x = minX; x <= maxX; x += 1) {
      const dx = x + 0.5 - centerX;
      const dy = y + 0.5 - centerY;
      const dist2 = dx * dx + dy * dy;
      if (dist2 > r2) continue;
      const index = (y * width + x) * 4;
      // Soft falloff outside the hard core: 1 at center/core, 0 at edge.
      let strength = 1;
      if (dist2 > softStart2 && r > softStart) {
        const dist = Math.sqrt(dist2);
        strength = 1 - (dist - softStart) / (r - softStart);
        strength = Math.max(0, Math.min(1, strength));
      }
      if (strength <= 0) continue;

      if (mode === "erase") {
        const before = data[index + 3];
        if (before === 0) continue;
        const nextAlpha = Math.round(before * (1 - strength));
        if (nextAlpha === before) continue;
        if (nextAlpha === 0) {
          clearPixel(data, index);
        } else {
          data[index + 3] = nextAlpha;
        }
        changed += 1;
      } else if (original) {
        const src = original.data;
        const afterR = Math.round(data[index] + (src[index] - data[index]) * strength);
        const afterG = Math.round(data[index + 1] + (src[index + 1] - data[index + 1]) * strength);
        const afterB = Math.round(data[index + 2] + (src[index + 2] - data[index + 2]) * strength);
        const afterA = Math.round(data[index + 3] + (src[index + 3] - data[index + 3]) * strength);
        if (
          afterR === data[index]
          && afterG === data[index + 1]
          && afterB === data[index + 2]
          && afterA === data[index + 3]
        ) {
          continue;
        }
        data[index] = afterR;
        data[index + 1] = afterG;
        data[index + 2] = afterB;
        data[index + 3] = afterA;
        changed += 1;
      }
    }
  }

  return changed;
}

/** Stamp a brush along a segment so fast strokes stay continuous. */
export function strokeRemoveBackgroundBrush(
  working: ImageData,
  fromX: number,
  fromY: number,
  toX: number,
  toY: number,
  radius: number,
  mode: "erase" | "restore",
  original: ImageData | null = null,
  hardness = DEFAULT_BRUSH_HARDNESS,
): number {
  const dx = toX - fromX;
  const dy = toY - fromY;
  const distance = Math.hypot(dx, dy);
  const step = Math.max(0.5, radius * 0.35);
  if (distance < 0.001) {
    return stampRemoveBackgroundBrush(
      working,
      toX,
      toY,
      radius,
      mode,
      original,
      hardness,
    );
  }
  let changed = 0;
  const steps = Math.max(1, Math.ceil(distance / step));
  for (let index = 0; index <= steps; index += 1) {
    const t = index / steps;
    changed += stampRemoveBackgroundBrush(
      working,
      fromX + dx * t,
      fromY + dy * t,
      radius,
      mode,
      original,
      hardness,
    );
  }
  return changed;
}

/** Rasterize an HTML image into a full-resolution ImageData buffer. */
export function imageToImageData(image: CanvasImageSource & {
  naturalWidth?: number;
  naturalHeight?: number;
  width: number;
  height: number;
}): ImageData {
  const width = Math.max(
    1,
    Math.round(image.naturalWidth ?? (image as HTMLImageElement).width),
  );
  const height = Math.max(
    1,
    Math.round(image.naturalHeight ?? (image as HTMLImageElement).height),
  );
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) {
    throw new Error("Could not read image pixels for the eraser.");
  }
  context.clearRect(0, 0, width, height);
  context.drawImage(image, 0, 0, width, height);
  try {
    return context.getImageData(0, 0, width, height);
  } catch {
    throw new Error(
      "This image cannot be edited with the eraser (protected or still loading).",
    );
  }
}

/** Write ImageData into a canvas (creates one when omitted). */
export function imageDataToCanvas(
  imageData: ImageData,
  canvas?: HTMLCanvasElement,
): HTMLCanvasElement {
  const target = canvas ?? document.createElement("canvas");
  if (target.width !== imageData.width) target.width = imageData.width;
  if (target.height !== imageData.height) target.height = imageData.height;
  const context = target.getContext("2d");
  if (!context) {
    throw new Error("Could not write image pixels for the eraser.");
  }
  context.putImageData(imageData, 0, 0);
  return target;
}

/** Encode ImageData as a PNG data URL for layer `src` replacement. */
export function imageDataToPngDataUrl(imageData: ImageData): string {
  const canvas = imageDataToCanvas(imageData);
  return canvas.toDataURL("image/png");
}

/**
 * Prepare an image layer after an alpha edit: keep a stable original for restore,
 * swap in the new PNG, and clear solid canvas fill so transparency is visible.
 */
export function applyImageBackgroundEdit(
  document: {
    background: string | null;
    elements: ScreenshotElement[];
    width: number;
    height: number;
  },
  elementId: string,
  nextSrc: string,
  sourceBeforeEdit: string,
): typeof document {
  const elements = document.elements.map((element) => {
    if (element.id !== elementId || element.kind !== "image") return element;
    return {
      ...element,
      src: nextSrc,
      // First alpha edit freezes the pre-edit bitmap for the restore brush.
      originalSrc: element.originalSrc ?? sourceBeforeEdit,
    };
  });
  return {
    ...document,
    // Transparent holes are useless under a solid fill — match export intent.
    background: null,
    elements,
  };
}
