/** Movement before a collapsed-pile press becomes a window drag instead of expand. */
export const THUMBNAIL_STACK_DRAG_THRESHOLD_PX = 8;

export const THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX = 12;
export const THUMBNAIL_STACK_DRAG_SWAY_MAX_Y_PX = 6;

/** Harness-only: CSS translation of `#root` from its default bottom-left strip. */
export const THUMBNAIL_HARNESS_DRAG_X_VAR = "--thumbnail-stack-drag-x";
export const THUMBNAIL_HARNESS_DRAG_Y_VAR = "--thumbnail-stack-drag-y";

export const THUMBNAIL_DRAG_SWAY_X_VAR = "--thumbnail-drag-sway-x";
export const THUMBNAIL_DRAG_SWAY_Y_VAR = "--thumbnail-drag-sway-y";

export const THUMBNAIL_STACK_DRAGGING_CLASS = "thumbnail-stack-dragging";

/**
 * Collapsed screenshots stay in the DOM as `<img>` drag sources. Chromium can
 * start a URL/file drag through a transparent overlay, which steals pointer
 * events and leaves the pile stuck. Cancel that so the stack can move.
 */
export function preventThumbnailHtml5Drag(event: Event): void {
  event.preventDefault();
  event.stopPropagation();
}

/** CSS `url()` with quotes escaped, for painting a preview without an `<img>`. */
export function cssUrl(value: string): string {
  return `url(${JSON.stringify(value)})`;
}

const HARNESS_FRAME_WIDTH_PX = 340;
const HARNESS_COLLAPSED_HEIGHT_PX = 240;

export type ThumbnailStackPoint = { x: number; y: number };

export type ThumbnailStackWorkArea = {
  x: number;
  y: number;
  width: number;
  height: number;
  bottomGap: number;
};

export type ThumbnailStackDragHost = {
  getFrame: () => ThumbnailStackPoint | Promise<ThumbnailStackPoint>;
  moveFrame: (x: number, y: number) => ThumbnailStackPoint | Promise<ThumbnailStackPoint>;
  reducedMotion: () => boolean;
};

export type ThumbnailStackDragMove = {
  dragging: boolean;
  x: number;
  y: number;
  sway: ThumbnailStackPoint;
};

function clamp(value: number, min: number, max: number): number {
  const next = Math.min(max, Math.max(min, value));
  return next === 0 ? 0 : next;
}

export function parseCssPx(value: string): number {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : 0;
}

export function thumbnailStackDragExceededThreshold(
  dx: number,
  dy: number,
  threshold: number = THUMBNAIL_STACK_DRAG_THRESHOLD_PX,
): boolean {
  return Math.hypot(dx, dy) >= threshold;
}

/**
 * Rear-card trail while the pile is carried. Front card (depth 0) stays glued
 * to the pointer; deeper cards lag opposite the drag, like a short stack of
 * paper.
 */
export function thumbnailStackDragSway(
  dx: number,
  dy: number,
  options: { reducedMotion?: boolean } = {},
): ThumbnailStackPoint {
  if (options.reducedMotion) return { x: 0, y: 0 };
  return {
    x: clamp(
      -dx * 0.4,
      -THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX,
      THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX,
    ),
    y: clamp(
      -dy * 0.18,
      -THUMBNAIL_STACK_DRAG_SWAY_MAX_Y_PX,
      THUMBNAIL_STACK_DRAG_SWAY_MAX_Y_PX,
    ),
  };
}

export function clampThumbnailStackFrame(
  x: number,
  y: number,
  frameWidth: number,
  frameHeight: number,
  work: ThumbnailStackWorkArea,
): ThumbnailStackPoint {
  const minX = work.x;
  const maxX = Math.max(minX, work.x + work.width - frameWidth);
  const minY = work.y;
  const maxY = Math.max(minY, work.y + work.height - work.bottomGap - frameHeight);
  return {
    x: clamp(x, minX, maxX),
    y: clamp(y, minY, maxY),
  };
}

export function readHarnessStackOffset(
  root: HTMLElement = document.documentElement,
): ThumbnailStackPoint {
  return {
    x: parseCssPx(root.style.getPropertyValue(THUMBNAIL_HARNESS_DRAG_X_VAR)),
    y: parseCssPx(root.style.getPropertyValue(THUMBNAIL_HARNESS_DRAG_Y_VAR)),
  };
}

export function writeHarnessStackOffset(
  x: number,
  y: number,
  root: HTMLElement = document.documentElement,
  viewport: { width: number; height: number } = {
    width: window.innerWidth,
    height: window.innerHeight,
  },
): ThumbnailStackPoint {
  const clamped = {
    x: clamp(x, 0, Math.max(0, viewport.width - HARNESS_FRAME_WIDTH_PX)),
    y: clamp(y, Math.min(0, HARNESS_COLLAPSED_HEIGHT_PX - viewport.height), 0),
  };
  root.style.setProperty(THUMBNAIL_HARNESS_DRAG_X_VAR, `${clamped.x}px`);
  root.style.setProperty(THUMBNAIL_HARNESS_DRAG_Y_VAR, `${clamped.y}px`);
  return clamped;
}

export function applyThumbnailStackDragSway(
  stack: HTMLElement | null,
  sway: ThumbnailStackPoint,
) {
  if (!stack) return;
  stack.style.setProperty(THUMBNAIL_DRAG_SWAY_X_VAR, String(sway.x));
  stack.style.setProperty(THUMBNAIL_DRAG_SWAY_Y_VAR, String(sway.y));
}

export function clearThumbnailStackDragSway(stack: HTMLElement | null) {
  applyThumbnailStackDragSway(stack, { x: 0, y: 0 });
}

export function setThumbnailStackDragging(stack: HTMLElement | null, dragging: boolean) {
  if (!stack) return;
  stack.classList.toggle(THUMBNAIL_STACK_DRAGGING_CLASS, dragging);
  if (!dragging) clearThumbnailStackDragSway(stack);
}

/**
 * Click-versus-drag session for the collapsed pile. Coordinates are CSS pixels
 * relative to the frame's top-left at pointer-down.
 */
export class CollapsedThumbnailStackDrag {
  private pointerId: number | null = null;
  private startPointer: ThumbnailStackPoint = { x: 0, y: 0 };
  private startFrame: ThumbnailStackPoint = { x: 0, y: 0 };
  private ready: Promise<void> | null = null;
  private dragging = false;

  constructor(private readonly host: ThumbnailStackDragHost) {}

  get isDragging(): boolean {
    return this.dragging;
  }

  get isActive(): boolean {
    return this.pointerId !== null;
  }

  pointerDown(event: Pick<PointerEvent, "button" | "pointerId" | "screenX" | "screenY">): boolean {
    if (event.button !== 0) return false;
    this.pointerId = event.pointerId;
    this.startPointer = { x: event.screenX, y: event.screenY };
    this.dragging = false;
    this.ready = Promise.resolve(this.host.getFrame()).then((frame) => {
      this.startFrame = frame;
    });
    return true;
  }

  async pointerMove(
    event: Pick<PointerEvent, "pointerId" | "screenX" | "screenY">,
  ): Promise<ThumbnailStackDragMove | null> {
    if (this.pointerId !== event.pointerId) return null;
    await this.ready;
    const dx = event.screenX - this.startPointer.x;
    const dy = event.screenY - this.startPointer.y;
    if (!this.dragging && !thumbnailStackDragExceededThreshold(dx, dy)) {
      return {
        dragging: false,
        x: this.startFrame.x,
        y: this.startFrame.y,
        sway: { x: 0, y: 0 },
      };
    }
    this.dragging = true;
    const next = await this.host.moveFrame(this.startFrame.x + dx, this.startFrame.y + dy);
    return {
      dragging: true,
      x: next.x,
      y: next.y,
      sway: thumbnailStackDragSway(dx, dy, { reducedMotion: this.host.reducedMotion() }),
    };
  }

  async pointerUp(
    event: Pick<PointerEvent, "pointerId">,
  ): Promise<"expand" | "drop" | "ignored"> {
    if (this.pointerId !== event.pointerId) return "ignored";
    await this.ready;
    const expand = !this.dragging;
    this.pointerId = null;
    this.dragging = false;
    this.ready = null;
    return expand ? "expand" : "drop";
  }
}
