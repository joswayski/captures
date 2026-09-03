export interface SelectionPoint {
  x: number;
  y: number;
}

export interface SelectionRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type SelectionDragMode = "create" | "move" | "nw" | "ne" | "sw" | "se";

/** Panel presets for capture region aspect lock. */
export const REGION_ASPECT_PRESETS = [
  { value: "free", label: "Free" },
  { value: "1:1", label: "1 : 1" },
  { value: "4:3", label: "4 : 3" },
  { value: "3:2", label: "3 : 2" },
  { value: "16:9", label: "16 : 9" },
  { value: "9:16", label: "9 : 16" },
] as const;

export type RegionAspectPreset = (typeof REGION_ASPECT_PRESETS)[number]["value"];

export type DragSelectionOptions = {
  minimumSize?: number;
  /**
   * Fixed width/height ratio from the aspect selector (`16/9`, `1`, …).
   * `null` / omitted means freeform. Ignored when `forceSquare` is true.
   */
  aspectRatio?: number | null;
  /** Shift held: force a 1:1 square regardless of the selected aspect. */
  forceSquare?: boolean;
};

export function frontToBackWindows<T extends { z_order: number }>(windows: readonly T[]): T[] {
  return windows
    .map((window, index) => ({ index, window }))
    .sort((left, right) => (
      right.window.z_order - left.window.z_order
      || left.index - right.index
    ))
    .map(({ window }) => window);
}

/**
 * Frontmost window whose overlay-space rectangle contains `point`.
 *
 * Overlay coordinates are `(window.x - origin.x) / scale`. Uses half-open
 * edges so neighboring windows don't both claim a shared pixel.
 */
export function frontmostWindowAtPoint<T extends {
  z_order: number;
  x: number;
  y: number;
  width: number;
  height: number;
}>(
  windows: readonly T[],
  point: SelectionPoint,
  origin: { x: number; y: number },
  scale = 1,
): T | null {
  const safeScale = scale > 0 ? scale : 1;
  for (const window of frontToBackWindows(windows)) {
    if (window.width <= 0 || window.height <= 0) continue;
    const left = (window.x - origin.x) / safeScale;
    const top = (window.y - origin.y) / safeScale;
    const width = window.width / safeScale;
    const height = window.height / safeScale;
    if (
      point.x >= left
      && point.y >= top
      && point.x < left + width
      && point.y < top + height
    ) {
      return window;
    }
  }
  return null;
}

export function windowListingIsReady(windowsReady: boolean | undefined): boolean {
  return windowsReady !== false;
}

export type CapturePointerHitKind = "window" | "chrome";

/**
 * Frontmost capturable window or shell-chrome strip at `point`.
 *
 * Edge chrome stays in the hit-test list so a maximized app behind the menu
 * bar / taskbar does not steal the pointer. Hits on chrome are not window
 * captures; callers treat them as the display.
 */
export function frontmostCaptureTargetAtPoint<T extends {
  z_order: number;
  x: number;
  y: number;
  width: number;
  height: number;
}>(
  windows: readonly T[],
  shellChrome: readonly T[],
  point: SelectionPoint,
  origin: { x: number; y: number },
  scale = 1,
): { kind: CapturePointerHitKind; target: T } | null {
  type Tagged = T & { __captureHitKind: CapturePointerHitKind };
  const tagged: Tagged[] = [
    ...windows.map((target) => ({ ...target, __captureHitKind: "window" as const })),
    ...shellChrome.map((target) => ({ ...target, __captureHitKind: "chrome" as const })),
  ];
  const hit = frontmostWindowAtPoint(tagged, point, origin, scale);
  if (!hit) return null;
  return { kind: hit.__captureHitKind, target: hit };
}

export function selectionRect(start: SelectionPoint, end: SelectionPoint): SelectionRect {
  return {
    x: Math.min(start.x, end.x),
    y: Math.min(start.y, end.y),
    width: Math.abs(end.x - start.x),
    height: Math.abs(end.y - start.y),
  };
}

/**
 * Parse a preset like `"16:9"` into width/height. `"free"` and invalid values
 * return `null` (freeform).
 */
export function parseAspectRatioPreset(value: string): number | null {
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
 * Effective aspect for a drag: Shift always wins with 1:1; otherwise the
 * selector ratio (or freeform when unset).
 */
export function effectiveDragAspectRatio(
  aspectRatio: number | null | undefined,
  forceSquare: boolean,
): number | null {
  if (forceSquare) return 1;
  if (aspectRatio && Number.isFinite(aspectRatio) && aspectRatio > 0) return aspectRatio;
  return null;
}

/**
 * Refit a settled selection to a new aspect ratio immediately (dropdown change).
 *
 * Keeps the previous center and fits inside the previous box so the region
 * never expands past what the user already framed. Returns `rect` unchanged
 * when `aspectRatio` is freeform/`null`.
 */
export function constrainSelectionToAspect(
  rect: SelectionRect,
  aspectRatio: number | null,
  bounds?: { width: number; height: number },
  minimumSize = 16,
): SelectionRect {
  if (aspectRatio === null || !Number.isFinite(aspectRatio) || aspectRatio <= 0) {
    return rect;
  }
  if (!(rect.width > 0) || !(rect.height > 0)) {
    return rect;
  }

  let width: number;
  let height: number;
  if (rect.width / rect.height > aspectRatio) {
    height = rect.height;
    width = height * aspectRatio;
  } else {
    width = rect.width;
    height = width / aspectRatio;
  }

  // Prefer a capturable minimum when the original box is large enough.
  const min = Math.max(1, minimumSize);
  if (width < min || height < min) {
    if (aspectRatio >= 1) {
      width = Math.max(min, width);
      height = width / aspectRatio;
    } else {
      height = Math.max(min, height);
      width = height * aspectRatio;
    }
  }

  let x = rect.x + (rect.width - width) / 2;
  let y = rect.y + (rect.height - height) / 2;

  if (bounds && bounds.width > 0 && bounds.height > 0) {
    if (width > bounds.width) {
      width = bounds.width;
      height = width / aspectRatio;
    }
    if (height > bounds.height) {
      height = bounds.height;
      width = height * aspectRatio;
    }
    x = clamp(x, 0, Math.max(0, bounds.width - width));
    y = clamp(y, 0, Math.max(0, bounds.height - height));
  }

  return { x, y, width, height };
}

export function isCapturableSelection(
  rect: SelectionRect | null,
): rect is SelectionRect {
  return rect !== null && rect.width >= 2 && rect.height >= 2;
}

export function roundedRectPath(rect: SelectionRect, cornerRadius: number): string {
  const left = rect.x;
  const top = rect.y;
  const right = rect.x + rect.width;
  const bottom = rect.y + rect.height;
  const radius = Math.max(
    0,
    Math.min(cornerRadius, rect.width / 2, rect.height / 2),
  );
  if (radius === 0) {
    return `M${left} ${top}H${right}V${bottom}H${left}Z`;
  }
  return [
    `M${left + radius} ${top}`,
    `H${right - radius}`,
    `A${radius} ${radius} 0 0 1 ${right} ${top + radius}`,
    `V${bottom - radius}`,
    `A${radius} ${radius} 0 0 1 ${right - radius} ${bottom}`,
    `H${left + radius}`,
    `A${radius} ${radius} 0 0 1 ${left} ${bottom - radius}`,
    `V${top + radius}`,
    `A${radius} ${radius} 0 0 1 ${left + radius} ${top}`,
    "Z",
  ].join("");
}

/**
 * CSS clip-path for a full-surface shade with a rectangular hole.
 *
 * Uses the same CSS pixel space as the selection marquee (`left`/`top`/`width`/
 * `height` on a positioned box). Prefer this over an SVG viewBox cutout when
 * the hole is square: on Windows, theoretical display DIPs can disagree with
 * the live WebView client size, which misaligned SVG path units against the
 * marquee.
 *
 * The hole ring must close back to its start. CSS `polygon()` auto-closes the
 * whole path to the first point; without an explicit hole close, that edge
 * runs from the hole's last corner to the screen origin and evenodd leaves a
 * bright diagonal "spotlight" from the top-left into the selection.
 */
export function captureDimClipPath(rect: SelectionRect): string {
  const left = Math.max(0, rect.x);
  const top = Math.max(0, rect.y);
  const right = left + Math.max(0, rect.width);
  const bottom = top + Math.max(0, rect.height);
  return [
    "polygon(evenodd,",
    "0% 0%, 100% 0%, 100% 100%, 0% 100%, 0% 0%,",
    `${left}px ${top}px, ${left}px ${bottom}px, ${right}px ${bottom}px, ${right}px ${top}px, ${left}px ${top}px)`,
  ].join(" ");
}

export function dragSelectionRect(
  mode: SelectionDragMode,
  origin: SelectionPoint,
  current: SelectionPoint,
  initial: SelectionRect,
  bounds: { width: number; height: number },
  options: DragSelectionOptions = {},
): SelectionRect {
  const minimumSize = options.minimumSize ?? 16;
  const aspect = effectiveDragAspectRatio(options.aspectRatio, Boolean(options.forceSquare));

  if (mode === "create") {
    return createSelectionRect(origin, current, bounds, aspect);
  }

  const dx = current.x - origin.x;
  const dy = current.y - origin.y;
  if (mode === "move") {
    return {
      ...initial,
      x: clamp(initial.x + dx, 0, Math.max(0, bounds.width - initial.width)),
      y: clamp(initial.y + dy, 0, Math.max(0, bounds.height - initial.height)),
    };
  }

  if (aspect !== null) {
    return resizeCornerWithAspect(mode, current, initial, bounds, aspect, minimumSize);
  }

  let left = initial.x;
  let top = initial.y;
  let right = initial.x + initial.width;
  let bottom = initial.y + initial.height;
  if (mode.includes("w")) left = clamp(initial.x + dx, 0, right - minimumSize);
  if (mode.includes("e")) right = clamp(initial.x + initial.width + dx, left + minimumSize, bounds.width);
  if (mode.includes("n")) top = clamp(initial.y + dy, 0, bottom - minimumSize);
  if (mode.includes("s")) bottom = clamp(initial.y + initial.height + dy, top + minimumSize, bounds.height);
  return { x: left, y: top, width: right - left, height: bottom - top };
}

/**
 * Create a region from origin→current, optionally locked to `aspectRatio`.
 * Mirrors photo-editor crop drag: the free corner follows the pointer while
 * width/height stay proportional, clamped to the display.
 */
function createSelectionRect(
  origin: SelectionPoint,
  current: SelectionPoint,
  bounds: { width: number; height: number },
  aspectRatio: number | null,
): SelectionRect {
  const start = {
    x: clamp(origin.x, 0, bounds.width),
    y: clamp(origin.y, 0, bounds.height),
  };
  let end = {
    x: clamp(current.x, 0, bounds.width),
    y: clamp(current.y, 0, bounds.height),
  };

  if (aspectRatio === null) {
    return selectionRect(start, end);
  }

  const directionX = end.x < start.x ? -1 : 1;
  const directionY = end.y < start.y ? -1 : 1;
  let width = Math.abs(end.x - start.x);
  let height = Math.abs(end.y - start.y);
  if (height === 0 || width / height > aspectRatio) {
    height = width / aspectRatio;
  } else {
    width = height * aspectRatio;
  }
  width = Math.min(width, directionX > 0 ? bounds.width - start.x : start.x);
  height = width / aspectRatio;
  if (height > (directionY > 0 ? bounds.height - start.y : start.y)) {
    height = directionY > 0 ? bounds.height - start.y : start.y;
    width = height * aspectRatio;
  }
  end = {
    x: start.x + width * directionX,
    y: start.y + height * directionY,
  };
  return selectionRect(start, end);
}

/**
 * Corner resize with a fixed opposite corner and locked aspect ratio.
 * The free corner tracks the pointer; size is clamped to the display.
 */
function resizeCornerWithAspect(
  mode: Exclude<SelectionDragMode, "create" | "move">,
  current: SelectionPoint,
  initial: SelectionRect,
  bounds: { width: number; height: number },
  aspect: number,
  minimumSize: number,
): SelectionRect {
  const anchor = {
    x: mode.includes("w") ? initial.x + initial.width : initial.x,
    y: mode.includes("n") ? initial.y + initial.height : initial.y,
  };
  const pointer = {
    x: clamp(current.x, 0, bounds.width),
    y: clamp(current.y, 0, bounds.height),
  };

  // Prefer the pointer quadrant so the free corner can flip past the anchor.
  const signX = pointer.x >= anchor.x ? 1 : -1;
  const signY = pointer.y >= anchor.y ? 1 : -1;

  let width = Math.max(Math.abs(pointer.x - anchor.x), 1e-6);
  let height = Math.max(Math.abs(pointer.y - anchor.y), 1e-6);
  if (width / height > aspect) {
    height = width / aspect;
  } else {
    width = height * aspect;
  }

  // Room available from the anchor in the chosen direction.
  const maxWidth = signX > 0 ? bounds.width - anchor.x : anchor.x;
  const maxHeight = signY > 0 ? bounds.height - anchor.y : anchor.y;
  if (width > maxWidth) {
    width = maxWidth;
    height = width / aspect;
  }
  if (height > maxHeight) {
    height = maxHeight;
    width = height * aspect;
  }

  // Keep both axes at least `minimumSize` without breaking the aspect when space allows.
  const min = Math.max(1, minimumSize);
  if ((width < min || height < min) && maxWidth >= min && maxHeight >= min / aspect && maxHeight >= min && maxWidth >= min * aspect) {
    if (aspect >= 1) {
      width = Math.max(min, Math.min(width, maxWidth));
      height = width / aspect;
      if (height < min || height > maxHeight) {
        height = Math.max(min, Math.min(height, maxHeight));
        width = height * aspect;
      }
    } else {
      height = Math.max(min, Math.min(height, maxHeight));
      width = height * aspect;
      if (width < min || width > maxWidth) {
        width = Math.max(min, Math.min(width, maxWidth));
        height = width / aspect;
      }
    }
  }

  // Degenerate: no room for a capturable rect in this quadrant — fall back to min fit.
  width = Math.max(0, Math.min(width, maxWidth));
  height = width / aspect;
  if (height > maxHeight) {
    height = Math.max(0, maxHeight);
    width = height * aspect;
  }

  return {
    x: signX > 0 ? anchor.x : anchor.x - width,
    y: signY > 0 ? anchor.y : anchor.y - height,
    width,
    height,
  };
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), Math.max(minimum, maximum));
}
