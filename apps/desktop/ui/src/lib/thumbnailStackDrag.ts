import { THUMBNAIL_CARD_HEIGHT_PX, THUMBNAIL_STACK_CONTROL_GUTTER_PX } from "./thumbnailLayout";

/** Movement before a collapsed-pile press becomes a window drag instead of expand. */
export const THUMBNAIL_STACK_DRAG_THRESHOLD_PX = 8;

export const THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX = 3;
export const THUMBNAIL_STACK_DRAG_SWAY_MAX_Y_PX = 2;

/**
 * The carried pile is deliberately under-damped: when the pointer pauses, the
 * rear cards briefly pass their rest position before settling instead of
 * stopping like a rigid object.
 */
export const THUMBNAIL_STACK_DRAG_SWAY_WOBBLE_SPRING = 500;
export const THUMBNAIL_STACK_DRAG_SWAY_WOBBLE_DAMPING = 32;
/** Converts cursor speed in CSS pixels per second into the trailing-card pose. */
export const THUMBNAIL_STACK_DRAG_SWAY_POINTER_SPEED_GAIN = 0.0012;
/** The pile picks up movement quickly, then releases it gently when the pointer stops. */
export const THUMBNAIL_STACK_DRAG_SWAY_DRIVE_IN_RATE = 55;
export const THUMBNAIL_STACK_DRAG_SWAY_DRIVE_OUT_RATE = 10;

/** First sample after a press has no previous timestamp; treat it as one frame. */
const THUMBNAIL_STACK_DRAG_SWAY_DEFAULT_DT_MS = 16;

/** Ignore huge pauses so a backgrounded tab cannot snap the pile. */
const THUMBNAIL_STACK_DRAG_SWAY_MAX_DT_MS = 48;

/** Stop repainting once the remaining pose is well below a visible subpixel. */
const THUMBNAIL_STACK_DRAG_SWAY_POSITION_EPSILON_PX = 0.001;
const THUMBNAIL_STACK_DRAG_SWAY_VELOCITY_EPSILON_PX_PER_SECOND = 0.01;

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
  /** Convert the dropped frame's anchor before another press can read its origin. */
  onDrop?: () => void | Promise<void>;
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

type ThumbnailStackDragSwayState = {
  position: ThumbnailStackPoint;
  velocity: ThumbnailStackPoint;
  drive: ThumbnailStackPoint;
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
 * Advance the live pile with a filtered velocity target and an under-damped
 * spring. The target picks up a carry quickly but eases away after it stops,
 * preventing the first pointer sample from kicking the pile into a jump.
 */
function tickThumbnailStackDragSwayState(
  sway: ThumbnailStackDragSwayState,
  motion: ThumbnailStackDragSwayMotion,
  options: { reducedMotion?: boolean } = {},
): ThumbnailStackDragSwayState {
  if (options.reducedMotion) {
    return { position: { x: 0, y: 0 }, velocity: { x: 0, y: 0 }, drive: { x: 0, y: 0 } };
  }
  const dt = swayDtMs(motion.dtMs) / 1000;
  if (dt === 0) return sway;
  const advance = (
    position: number,
    velocity: number,
    drive: number,
    pointerStep: number,
    max: number,
  ) => {
    const pointerSpeed = pointerStep / dt;
    const desiredDrive = clamp(
      -pointerSpeed * THUMBNAIL_STACK_DRAG_SWAY_POINTER_SPEED_GAIN,
      -max,
      max,
    );
    const driveRate = pointerStep === 0
      ? THUMBNAIL_STACK_DRAG_SWAY_DRIVE_OUT_RATE
      : THUMBNAIL_STACK_DRAG_SWAY_DRIVE_IN_RATE;
    const nextDrive = drive + (desiredDrive - drive) * (1 - Math.exp(-driveRate * dt));
    const nextVelocity = (
      velocity * Math.exp(-THUMBNAIL_STACK_DRAG_SWAY_WOBBLE_DAMPING * dt)
      + (nextDrive - position) * THUMBNAIL_STACK_DRAG_SWAY_WOBBLE_SPRING * dt
    );
    const nextPosition = clamp(position + nextVelocity * dt, -max, max);
    // A clamp is a physical boundary, not stored momentum waiting to kick the
    // pile back into motion on the next frame.
    return nextPosition === -max || nextPosition === max
      ? { position: nextPosition, velocity: 0, drive: nextDrive }
      : { position: nextPosition, velocity: nextVelocity, drive: nextDrive };
  };
  const x = advance(
    sway.position.x,
    sway.velocity.x,
    sway.drive.x,
    motion.dx,
    THUMBNAIL_STACK_DRAG_SWAY_MAX_X_PX,
  );
  const y = advance(
    sway.position.y,
    sway.velocity.y,
    sway.drive.y,
    motion.dy,
    THUMBNAIL_STACK_DRAG_SWAY_MAX_Y_PX,
  );
  return {
    position: { x: x.position, y: y.position },
    velocity: { x: x.velocity, y: y.velocity },
    drive: { x: x.drive, y: y.drive },
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
  padding?: number,
): ThumbnailStackPoint {
  if (padding !== undefined) {
    const frontY = y + frameHeight - padding - THUMBNAIL_CARD_HEIGHT_PX;
    const virtualY = anchor === "top"
      ? frontY - THUMBNAIL_STACK_CONTROL_GUTTER_PX
      : frontY + THUMBNAIL_CARD_HEIGHT_PX + THUMBNAIL_STACK_CONTROL_GUTTER_PX - frameHeight;
    const next = clampThumbnailStackFrame(
      x, virtualY, frameWidth, frameHeight, work, contentHeight, anchor,
    );
    return { x: next.x, y: y + next.y - virtualY };
  }
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

/**
 * Native inner size, or the webview height when that API is unavailable.
 * Never fall back to the collapsed content box alone: a preserved expanded
 * frame would then clamp as if it were 240px and pin the pile to the bar top.
 */
export function thumbnailStackMeasuredFrameHeight(
  measuredHeight: number | null | undefined,
  contentHeight: number,
  viewportHeight: number,
): number {
  if (typeof measuredHeight === "number" && measuredHeight > 0) {
    return measuredHeight;
  }
  return Math.max(contentHeight, Math.max(0, viewportHeight));
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
  padding?: number;
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
  const clamped = options.padding === undefined ? {
    x: clamp(x, 0, Math.max(0, viewport.width - HARNESS_FRAME_WIDTH_PX)),
    y: clamp(y, minY, maxY),
  } : clampThumbnailStackFrame(
    x, y, HARNESS_FRAME_WIDTH_PX, viewport.height,
    { x: 0, y: 0, width: viewport.width, height: viewport.height, bottomGap: 0 },
    contentHeight, anchor, options.padding,
  );
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
  private releasing = false;
  private sway: ThumbnailStackPoint = { x: 0, y: 0 };
  private swayVelocity: ThumbnailStackPoint = { x: 0, y: 0 };
  private swayDrive: ThumbnailStackPoint = { x: 0, y: 0 };
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

  /** Start lean from rest after the hover fan has gathered. */
  resetSway() {
    this.sway = { x: 0, y: 0 };
    this.swayVelocity = { x: 0, y: 0 };
    this.swayDrive = { x: 0, y: 0 };
    this.lastTickMs = 0;
    this.host.onSway?.(this.sway);
  }

  pointerDown(event: Pick<PointerEvent, "button" | "pointerId" | "screenX" | "screenY">): boolean {
    if (event.button !== 0 || this.isActive) return false;
    this.beginSession(event.pointerId, event.screenX, event.screenY);
    return true;
  }

  async pointerMove(
    event: Pick<PointerEvent, "pointerId" | "screenX" | "screenY">,
  ): Promise<ThumbnailStackDragMove | null> {
    if (this.pointerId !== event.pointerId || this.releasing) return null;
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
    if (this.pointerId !== event.pointerId || this.releasing) return "ignored";
    const session = this.session;
    this.releasing = true;
    try {
      await this.ready;
      if (!this.sessionIs(session, event.pointerId)) return "ignored";
      const expand = !this.dragging;
      if (!expand) {
        await this.flushDropMove(session, event.pointerId);
        await this.host.onDrop?.();
      }
      return expand ? "expand" : "drop";
    } finally {
      // Native IPC can fail if the last capture disappears or a display goes
      // away. Never leave the session (and its sway RAF) locked in that case.
      if (this.sessionIs(session, event.pointerId)) this.endSession();
    }
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
    this.swayVelocity = { x: 0, y: 0 };
    this.swayDrive = { x: 0, y: 0 };
    this.lastTickMs = 0;
    this.pointerSampled = false;
    this.stopSwayLoop();
    this.ready = Promise.resolve().then(() => this.host.getFrame()).then((frame) => {
      if (session !== this.session) return;
      this.startFrame = frame;
    });
    // Observe failures even before the first move/up awaits the frame. Keep
    // the rejected promise so pointerUp can release the session without expanding.
    void this.ready.catch(() => undefined);
  }

  /**
   * Let the latest serialized sample land, then write lastPointer once more
   * so settlement sees the drop position. Invalidating the session first would
   * skip a queued sample and let an already-started native move finish after
   * the top/bottom anchor conversion.
   */
  private async flushDropMove(session: number, pointerId: number) {
    await this.moveTail;
    if (!this.sessionIs(session, pointerId) || !this.dragging) return;
    await this.host.moveFrame(
      this.startFrame.x + (this.lastPointer.x - this.startPointer.x),
      this.startFrame.y + (this.lastPointer.y - this.startPointer.y),
    );
  }

  private endSession() {
    this.session += 1;
    this.moveGeneration += 1;
    this.pointerId = null;
    this.dragging = false;
    this.releasing = false;
    this.ready = null;
    this.stopSwayLoop();
    this.sway = { x: 0, y: 0 };
    this.swayVelocity = { x: 0, y: 0 };
    this.swayDrive = { x: 0, y: 0 };
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
    const next = tickThumbnailStackDragSwayState(
      { position: this.sway, velocity: this.swayVelocity, drive: this.swayDrive },
      { dx, dy, dtMs },
      { reducedMotion: this.host.reducedMotion() },
    );
    this.sway = next.position;
    this.swayVelocity = next.velocity;
    this.swayDrive = next.drive;
  }

  private startSwayLoop() {
    if (this.swayRaf !== 0 || !this.host.onSway || this.host.reducedMotion()) return;
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
      if (this.swayIsSettled()) {
        this.sway = { x: 0, y: 0 };
        this.swayVelocity = { x: 0, y: 0 };
        this.swayDrive = { x: 0, y: 0 };
        this.lastTickMs = 0;
        this.swayRaf = 0;
        this.host.onSway?.(this.sway);
        return;
      }
      this.swayRaf = requestAnimationFrame(step);
    };
    this.swayRaf = requestAnimationFrame(step);
  }

  private swayIsSettled(): boolean {
    return Math.abs(this.sway.x) <= THUMBNAIL_STACK_DRAG_SWAY_POSITION_EPSILON_PX
      && Math.abs(this.sway.y) <= THUMBNAIL_STACK_DRAG_SWAY_POSITION_EPSILON_PX
      && Math.abs(this.swayDrive.x) <= THUMBNAIL_STACK_DRAG_SWAY_POSITION_EPSILON_PX
      && Math.abs(this.swayDrive.y) <= THUMBNAIL_STACK_DRAG_SWAY_POSITION_EPSILON_PX
      && Math.abs(this.swayVelocity.x) <= THUMBNAIL_STACK_DRAG_SWAY_VELOCITY_EPSILON_PX_PER_SECOND
      && Math.abs(this.swayVelocity.y) <= THUMBNAIL_STACK_DRAG_SWAY_VELOCITY_EPSILON_PX_PER_SECOND;
  }

  private stopSwayLoop() {
    if (this.swayRaf !== 0) cancelAnimationFrame(this.swayRaf);
    this.swayRaf = 0;
  }
}
