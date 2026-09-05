/** Movement before a collapsed-pile press becomes a window drag instead of expand. */
export const THUMBNAIL_STACK_DRAG_THRESHOLD_PX = 8;

export const THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX = 4;
export const THUMBNAIL_STACK_DRAG_SWAY_MAX_Y_PX = 2.5;

/**
 * How much of each pointer step the rear cards initially refuse to follow.
 * 1 would leave them frozen in world space; 0 would glue them to the hands.
 * Keep this low so a flick cannot throw peeking cards off the webview.
 */
export const THUMBNAIL_STACK_DRAG_SWAY_INERTIA = 0.18;

/** Catch-up rate in 1/seconds. Higher is a stiffer stack. */
export const THUMBNAIL_STACK_DRAG_SWAY_SPRING = 14;

/** First sample after a press has no previous timestamp; treat it as one frame. */
const THUMBNAIL_STACK_DRAG_SWAY_DEFAULT_DT_MS = 16;

/** Ignore huge pauses so a backgrounded tab cannot snap the pile. */
const THUMBNAIL_STACK_DRAG_SWAY_MAX_DT_MS = 48;

/** Harness-only: CSS translation of `#root` from its default bottom-left strip. */
export const THUMBNAIL_HARNESS_DRAG_X_VAR = "--thumbnail-stack-drag-x";
export const THUMBNAIL_HARNESS_DRAG_Y_VAR = "--thumbnail-stack-drag-y";

export const THUMBNAIL_DRAG_SWAY_X_VAR = "--thumbnail-drag-sway-x";
export const THUMBNAIL_DRAG_SWAY_Y_VAR = "--thumbnail-drag-sway-y";

export const THUMBNAIL_STACK_DRAGGING_CLASS = "thumbnail-stack-dragging";
export const THUMBNAIL_STACK_PRESSING_CLASS = "thumbnail-stack-pressing";
/** Live lean pose. Omitted while the hover fan is still easing back to rest. */
export const THUMBNAIL_STACK_DRAG_SWAY_CLASS = "thumbnail-stack-drag-sway";

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
  /** Live lag pose. Called from pointer samples and catch-up frames. */
  onSway?: (sway: ThumbnailStackPoint) => void;
  /** Fires as soon as the press becomes a drag, before the frame moves. */
  onDraggingChange?: (dragging: boolean) => void;
  now?: () => number;
};

export type ThumbnailStackDragMove = {
  dragging: boolean;
  x: number;
  y: number;
  sway: ThumbnailStackPoint;
};

export type ThumbnailStackDragSwayMotion = {
  dx: number;
  dy: number;
  dtMs: number;
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

function swayDtMs(dtMs: number): number {
  if (!Number.isFinite(dtMs) || dtMs <= 0) return 0;
  return Math.min(dtMs, THUMBNAIL_STACK_DRAG_SWAY_MAX_DT_MS);
}

/**
 * Rear-card trail while the pile is carried. The front card stays glued to the
 * pointer; this vector is the extra lag at depth 1. Deeper cards take a larger
 * share in CSS so the pile arches: the hands move first, the top follows late.
 *
 * `dx`/`dy` are the latest pointer step, not the total drag from press, so the
 * lean tracks velocity and settles when the pointer stops.
 */
export function tickThumbnailStackDragSway(
  sway: ThumbnailStackPoint,
  motion: ThumbnailStackDragSwayMotion,
  options: { reducedMotion?: boolean } = {},
): ThumbnailStackPoint {
  if (options.reducedMotion) return { x: 0, y: 0 };
  const dt = swayDtMs(motion.dtMs) / 1000;
  let x = sway.x - motion.dx * THUMBNAIL_STACK_DRAG_SWAY_INERTIA;
  let y = sway.y - motion.dy * THUMBNAIL_STACK_DRAG_SWAY_INERTIA;
  if (dt > 0) {
    const decay = Math.exp(-THUMBNAIL_STACK_DRAG_SWAY_SPRING * dt);
    x *= decay;
    y *= decay;
  }
  return {
    x: clamp(x, -THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX, THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX),
    y: clamp(y, -THUMBNAIL_STACK_DRAG_SWAY_MAX_Y_PX, THUMBNAIL_STACK_DRAG_SWAY_MAX_Y_PX),
  };
}

export function clampThumbnailStackFrame(
  x: number,
  y: number,
  frameWidth: number,
  frameHeight: number,
  work: ThumbnailStackWorkArea,
  contentHeight: number = frameHeight,
  anchor: "top" | "bottom" = "bottom",
): ThumbnailStackPoint {
  // macOS/Linux keep the collapsed window at its expanded height. Bottom piles
  // sit in the lower content box (empty chrome may leave the work area above);
  // top piles sit in the upper content box so peek-down has room below.
  const content = Math.min(frameHeight, Math.max(0, contentHeight));
  const slack = Math.max(0, frameHeight - content);
  const minX = work.x;
  const maxX = Math.max(minX, work.x + work.width - frameWidth);
  const minY = anchor === "top" ? work.y : work.y - slack;
  const maxY = Math.max(
    minY,
    anchor === "top"
      ? work.y + work.height - work.bottomGap - content
      : work.y + work.height - work.bottomGap - frameHeight,
  );
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

export type HarnessStackOffsetOptions = {
  anchor?: "top" | "bottom";
  contentHeight?: number;
};

export function writeHarnessStackOffset(
  x: number,
  y: number,
  root: HTMLElement = document.documentElement,
  viewport: { width: number; height: number } = {
    width: window.innerWidth,
    height: window.innerHeight,
  },
  options: HarnessStackOffsetOptions = {},
): ThumbnailStackPoint {
  const contentHeight = options.contentHeight ?? HARNESS_COLLAPSED_HEIGHT_PX;
  const anchor = options.anchor ?? "bottom";
  const minY = anchor === "top" ? 0 : Math.min(0, contentHeight - viewport.height);
  const maxY = anchor === "top"
    ? Math.max(0, viewport.height - contentHeight)
    : 0;
  const clamped = {
    x: clamp(x, 0, Math.max(0, viewport.width - HARNESS_FRAME_WIDTH_PX)),
    y: clamp(y, minY, maxY),
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
  if (!dragging) {
    setThumbnailStackDragSwayReady(stack, false);
    clearThumbnailStackDragSway(stack);
  }
}

export function setThumbnailStackPressing(stack: HTMLElement | null, pressing: boolean) {
  stack?.classList.toggle(THUMBNAIL_STACK_PRESSING_CLASS, pressing);
}

export function setThumbnailStackDragSwayReady(
  stack: HTMLElement | null,
  ready: boolean,
) {
  stack?.classList.toggle(THUMBNAIL_STACK_DRAG_SWAY_CLASS, ready);
}

/**
 * Click-versus-drag session for the collapsed pile. Coordinates are CSS pixels
 * relative to the frame's top-left at pointer-down.
 */
export class CollapsedThumbnailStackDrag {
  private pointerId: number | null = null;
  private session = 0;
  private startPointer: ThumbnailStackPoint = { x: 0, y: 0 };
  private lastPointer: ThumbnailStackPoint = { x: 0, y: 0 };
  private startFrame: ThumbnailStackPoint = { x: 0, y: 0 };
  private ready: Promise<void> | null = null;
  private dragging = false;
  private sway: ThumbnailStackPoint = { x: 0, y: 0 };
  private lastTickMs = 0;
  private swayRaf = 0;
  private pointerSampled = false;
  /** Bumps so a newer pointer sample can retire an in-flight frame move. */
  private moveGeneration = 0;
  private moveTail: Promise<void> = Promise.resolve();

  constructor(private readonly host: ThumbnailStackDragHost) {}

  get isDragging(): boolean {
    return this.dragging;
  }

  get isActive(): boolean {
    return this.pointerId !== null;
  }

  /**
   * Keep the pointer delta after a coordinate-system change (bottom↔top
   * harness anchor) so later moves do not jump back to the old origin.
   */
  rebaseFrame(frame: ThumbnailStackPoint) {
    this.startFrame = {
      x: frame.x - (this.lastPointer.x - this.startPointer.x),
      y: frame.y - (this.lastPointer.y - this.startPointer.y),
    };
  }

  /** Start lean from rest after the hover fan has gathered. */
  resetSway() {
    this.sway = { x: 0, y: 0 };
    this.lastTickMs = 0;
    this.host.onSway?.(this.sway);
  }

  pointerDown(event: Pick<PointerEvent, "button" | "pointerId" | "screenX" | "screenY">): boolean {
    if (event.button !== 0) return false;
    this.beginSession(event.pointerId, event.screenX, event.screenY);
    return true;
  }

  async pointerMove(
    event: Pick<PointerEvent, "pointerId" | "screenX" | "screenY">,
  ): Promise<ThumbnailStackDragMove | null> {
    if (this.pointerId !== event.pointerId) return null;
    const session = this.session;
    const stepX = event.screenX - this.lastPointer.x;
    const stepY = event.screenY - this.lastPointer.y;
    this.lastPointer = { x: event.screenX, y: event.screenY };
    const dx = event.screenX - this.startPointer.x;
    const dy = event.screenY - this.startPointer.y;
    if (!this.dragging && !thumbnailStackDragExceededThreshold(dx, dy)) {
      await this.ready;
      if (!this.sessionIs(session, event.pointerId)) return null;
      return {
        dragging: false,
        x: this.startFrame.x,
        y: this.startFrame.y,
        sway: { x: 0, y: 0 },
      };
    }
    const crossed = !this.dragging;
    this.dragging = true;
    if (crossed) this.host.onDraggingChange?.(true);
    this.tickSway(
      this.now(),
      crossed ? dx : stepX,
      crossed ? dy : stepY,
    );
    this.pointerSampled = true;
    this.startSwayLoop();
    this.host.onSway?.(this.sway);
    const generation = ++this.moveGeneration;
    const result = this.moveTail.then(async () => {
      if (!this.sessionIs(session, event.pointerId) || generation !== this.moveGeneration) {
        return null;
      }
      await this.ready;
      if (!this.sessionIs(session, event.pointerId) || generation !== this.moveGeneration) {
        return null;
      }
      const moveDx = this.lastPointer.x - this.startPointer.x;
      const moveDy = this.lastPointer.y - this.startPointer.y;
      const next = await this.host.moveFrame(
        this.startFrame.x + moveDx,
        this.startFrame.y + moveDy,
      );
      if (!this.sessionIs(session, event.pointerId) || generation !== this.moveGeneration) {
        return null;
      }
      return {
        dragging: true as const,
        x: next.x,
        y: next.y,
        sway: this.sway,
      };
    });
    this.moveTail = result.then(() => undefined, () => undefined);
    return result;
  }

  async pointerUp(
    event: Pick<PointerEvent, "pointerId">,
  ): Promise<"expand" | "drop" | "ignored"> {
    if (this.pointerId !== event.pointerId) return "ignored";
    const session = this.session;
    await this.ready;
    if (!this.sessionIs(session, event.pointerId)) return "ignored";
    const expand = !this.dragging;
    this.endSession();
    return expand ? "expand" : "drop";
  }

  private beginSession(pointerId: number, screenX: number, screenY: number) {
    this.session += 1;
    this.moveGeneration += 1;
    const session = this.session;
    this.pointerId = pointerId;
    this.startPointer = { x: screenX, y: screenY };
    this.lastPointer = this.startPointer;
    this.dragging = false;
    this.sway = { x: 0, y: 0 };
    this.lastTickMs = 0;
    this.pointerSampled = false;
    this.stopSwayLoop();
    this.ready = Promise.resolve(this.host.getFrame()).then((frame) => {
      if (session !== this.session) return;
      this.startFrame = frame;
    });
  }

  private endSession() {
    this.session += 1;
    this.moveGeneration += 1;
    this.pointerId = null;
    this.dragging = false;
    this.ready = null;
    this.stopSwayLoop();
    this.sway = { x: 0, y: 0 };
    this.lastTickMs = 0;
    this.pointerSampled = false;
  }

  private sessionIs(session: number, pointerId: number): boolean {
    return this.session === session && this.pointerId === pointerId;
  }

  private now(): number {
    return this.host.now?.() ?? performance.now();
  }

  private tickSway(now: number, dx: number, dy: number) {
    const dtMs = this.lastTickMs === 0
      ? THUMBNAIL_STACK_DRAG_SWAY_DEFAULT_DT_MS
      : now - this.lastTickMs;
    this.lastTickMs = now;
    this.sway = tickThumbnailStackDragSway(
      this.sway,
      { dx, dy, dtMs },
      { reducedMotion: this.host.reducedMotion() },
    );
  }

  private startSwayLoop() {
    if (this.swayRaf !== 0 || !this.host.onSway) return;
    const step = (now: number) => {
      if (this.pointerId === null || !this.dragging) {
        this.swayRaf = 0;
        return;
      }
      if (this.pointerSampled) {
        this.pointerSampled = false;
      } else {
        this.tickSway(now, 0, 0);
        this.host.onSway?.(this.sway);
      }
      this.swayRaf = requestAnimationFrame(step);
    };
    this.swayRaf = requestAnimationFrame(step);
  }

  private stopSwayLoop() {
    if (this.swayRaf !== 0) cancelAnimationFrame(this.swayRaf);
    this.swayRaf = 0;
  }
}
