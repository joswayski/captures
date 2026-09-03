/** Thumbnail card height in CSS pixels (matches `.thumbnail-card` flex-basis/height). */
export const THUMBNAIL_CARD_HEIGHT_PX = 160;

/** Vertical gap between cards (matches `.thumbnail-stack` gap). */
export const THUMBNAIL_STACK_GAP_PX = 24;

/** Top/side padding on `.thumbnail-stack`. */
export const THUMBNAIL_STACK_PADDING_PX = 28;

/** Bottom padding that reserves the expanded Show less gutter. */
export const THUMBNAIL_STACK_CONTROL_GUTTER_PX = 52;

/** One stack slot: card height + inter-card gap. */
export const THUMBNAIL_CARD_SLOT_PX = THUMBNAIL_CARD_HEIGHT_PX + THUMBNAIL_STACK_GAP_PX;

/** How many collapsed cards peek behind the front preview. */
export const THUMBNAIL_STACK_MAX_VISIBLE_DEPTH = 3;

/** Idle collapsed peek per extra card (matches compact `translateY`). */
export const THUMBNAIL_STACK_IDLE_PEEK_PX = 13;

/** Hover-fan collapsed peek per extra card. */
export const THUMBNAIL_STACK_HOVER_PEEK_PX = 24;

/** Extra hit-target height so a hovered tilt corner is still on the pile. */
export const THUMBNAIL_STACK_HOVER_TILT_PEEK_PX = 18;

/** Extra delay per stacked card so collapsed hover lift does not fire in lockstep. */
export const THUMBNAIL_STACK_FAN_STAGGER_MS = 8;

/**
 * Alternating collapsed-hover tilt in degrees. The front card stays square;
 * deeper cards skew a few degrees and ease back to 0 when the stack expands.
 * Values are large enough that the peeking top edge reads as a scattered pile.
 */
export const THUMBNAIL_STACK_FAN_TILT_DEG = [0, 7, -6, 5] as const;

/** Extra hover shift (px) along the tilt so the peeking edge is not a parallel slab. */
export const THUMBNAIL_STACK_FAN_SHIFT_PX_PER_DEG = 1.8;

/** Tilt applied to a collapsed card at `depth` while the pile is hovered. */
export function thumbnailStackFanTiltDeg(depth: number): number {
  const index = Math.min(
    Math.max(Math.trunc(depth), 0),
    THUMBNAIL_STACK_FAN_TILT_DEG.length - 1,
  );
  return THUMBNAIL_STACK_FAN_TILT_DEG[index];
}

/** Horizontal hover offset matching `thumbnailStackFanTiltDeg`. */
export function thumbnailStackFanShiftPx(depth: number): number {
  return thumbnailStackFanTiltDeg(depth) * THUMBNAIL_STACK_FAN_SHIFT_PX_PER_DEG;
}

const THUMBNAIL_CARD_ID_ATTRIBUTE = "data-thumbnail-id";

/**
 * Snapshot each collapsed card's live transform so expand can ease from a
 * latched rest pose, a mid-fan tween, or the full hover fan without snapping.
 */
export function captureThumbnailCardTransforms(
  stack: Element | null,
): Map<string, string> {
  const captured = new Map<string, string>();
  if (!stack) return captured;
  stack.querySelectorAll<HTMLElement>(":scope > .thumbnail-card").forEach((card) => {
    const id = card.getAttribute(THUMBNAIL_CARD_ID_ATTRIBUTE);
    if (!id) return;
    const transform = getComputedStyle(card).transform;
    if (!transform || transform === "none") return;
    captured.set(id, transform);
  });
  return captured;
}

/**
 * Extra height above the front card for the collapsed expand target.
 * One preview stays 160px so empty space above it still click-through.
 */
export function thumbnailCollapsedPeekPx(
  cardCount: number,
  hovered = false,
): number {
  const extra = Math.min(
    Math.max(cardCount - 1, 0),
    THUMBNAIL_STACK_MAX_VISIBLE_DEPTH,
  );
  const peek = extra * (hovered ? THUMBNAIL_STACK_HOVER_PEEK_PX : THUMBNAIL_STACK_IDLE_PEEK_PX);
  if (hovered && extra > 0) return peek + THUMBNAIL_STACK_HOVER_TILT_PEEK_PX;
  return peek;
}

/** Duration for one-slot overflow-cue scrolls (ease-out). */
export const THUMBNAIL_STACK_SCROLL_DURATION_MS = 380;

/**
 * Delay before live cards above a dust-delete begin sliding into the empty
 * slot. Matches the pre-motion ash phase in styles.css.
 */
export const THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS = 1_800;

/**
 * Delay before live cards above a dismiss begin sliding. Matches the point
 * where the outgoing preview has fully faded/streaked off-screen.
 */
export const THUMBNAIL_DISMISS_STACK_MOTION_DELAY_MS = 450;

/** @deprecated Prefer THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS. */
export const THUMBNAIL_STACK_MOTION_DELAY_MS = THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS;

/**
 * Shared settle duration for survivors after delete or dismiss.
 * Keep identical so both exit paths feel the same.
 */
export const THUMBNAIL_STACK_MOTION_DURATION_MS = 580;

/**
 * Bound any wait for a retargeted survivor transition. Two settle windows
 * cover a delete/close overlap while still guaranteeing cleanup if a hidden
 * WebView pauses its animations.
 */
export const THUMBNAIL_STACK_SETTLE_MAX_WAIT_MS =
  THUMBNAIL_STACK_MOTION_DURATION_MS * 2 + 100;

/**
 * How long a dismiss card keeps its layout slot (visual exit + stacked settle).
 * Matches the CSS `thumbnail-dismiss` animation duration.
 */
export const THUMBNAIL_DISMISS_HOLD_MS =
  THUMBNAIL_DISMISS_STACK_MOTION_DELAY_MS + THUMBNAIL_STACK_MOTION_DURATION_MS;

export type ThumbnailStackCardMotionState = {
  /** True while this card is locked in any exit animation. */
  exiting: boolean;
  /**
   * True when this card still occupies layout space and should pull cards
   * above it downward once `motionReady` (dust-delete or dismiss hold).
   */
  holdsLayoutSlot: boolean;
  /**
   * True once this exit's motion delay has elapsed so its slot contributes
   * to the stacked shift of live cards above it.
   */
  motionReady: boolean;
  /**
   * Shift already applied to this card, in CSS pixels. Exiting cards keep this
   * value instead of sliding into holes that opened after they started exiting.
   */
  currentShiftPx?: number;
};

/**
 * Count how many motion-ready held-layout exit slots sit below `index`.
 * Live cards slide by this many slots.
 */
export function countMotionReadySlotsBelow(
  cards: readonly ThumbnailStackCardMotionState[],
  index: number,
): number {
  let count = 0;
  for (let i = index + 1; i < cards.length; i += 1) {
    const card = cards[i];
    if (card?.holdsLayoutSlot && card.motionReady) count += 1;
  }
  return count;
}

/** @deprecated Prefer countMotionReadySlotsBelow. */
export const countMotionReadyDeleteSlotsBelow = countMotionReadySlotsBelow;

/** Pixel shift for a live card sitting above `slots` open exit holes. */
export function thumbnailStackShiftPx(slots: number): number {
  return Math.max(0, slots) * THUMBNAIL_CARD_SLOT_PX;
}

/**
 * Live cards follow the stacked hole distance. Exiting cards keep any shift
 * they already have so a delete/dismiss that starts after a settle does not
 * snap back to the untranslated layout slot; they never pick up holes that
 * opened after they started exiting.
 */
export function resolveThumbnailStackShiftPx(
  livePx: number,
  currentShiftPx: number,
  exiting: boolean,
): number {
  const live = Math.max(0, livePx);
  if (!exiting) return live;
  return Math.min(Math.max(0, currentShiftPx), live);
}

/** Treat a dissolving-in-place card as passable after its motion delay. */
function isClearExitHole(
  card: ThumbnailStackCardMotionState | undefined,
  resolvedShiftPx: number,
): boolean {
  return Boolean(card?.holdsLayoutSlot && card.motionReady && resolvedShiftPx <= 0.5);
}

/**
 * Compute the target translateY (px) for every card in order.
 * Live survivors slide into motion-ready holes; exiting cards keep the shift
 * they already had until those holes are removed from layout.
 *
 * Cards never close the gap to a neighbor that still occupies its slot. That
 * keeps a convoy when several live cards follow a lower hole, and it stops a
 * live card from sliding into a preview that started deleting mid-settle.
 * Dissolving-in-place holes (motion-ready, unshifted) stay passable so a
 * single delete still eases into the ash after the usual delay.
 */
export function computeThumbnailStackShifts(
  cards: readonly ThumbnailStackCardMotionState[],
): number[] {
  // Single bottom-up pass keeps this O(n); it runs from a MutationObserver
  // that can fire repeatedly during exit animations.
  const shifts = new Array<number>(cards.length);
  let readySlotsBelow = 0;
  let blockingPxFromBelow = Number.POSITIVE_INFINITY;
  for (let index = cards.length - 1; index >= 0; index -= 1) {
    const card = cards[index];
    const livePx = Math.min(thumbnailStackShiftPx(readySlotsBelow), blockingPxFromBelow);
    const resolvedPx = resolveThumbnailStackShiftPx(
      livePx,
      card?.currentShiftPx ?? 0,
      Boolean(card?.exiting),
    );
    shifts[index] = resolvedPx;
    if (isClearExitHole(card, resolvedPx)) {
      // Pass through this empty-looking slot; the next occupied card is one
      // more slot farther away.
      blockingPxFromBelow += THUMBNAIL_CARD_SLOT_PX;
    } else {
      blockingPxFromBelow = resolvedPx;
    }
    if (card?.holdsLayoutSlot && card.motionReady) readySlotsBelow += 1;
  }
  return shifts;
}

/**
 * Increases should ease so multi-exit stacks accumulate smoothly.
 * Decreases must snap: removing a finished exit reflows layout by one
 * slot, and an instant transform drop of the same amount cancels the jump.
 */
export function shouldAnimateThumbnailStackShift(
  previousPx: number,
  nextPx: number,
): boolean {
  return nextPx > previousPx;
}

export function shouldScrollThumbnailStackToEnd(
  previousCount: number,
  nextCount: number,
): boolean {
  return nextCount > previousCount;
}

/**
 * Expanding the pile starts at scrollTop 0 (oldest). Jump to the newest
 * capture once cards are back in document flow.
 */
export function shouldScrollThumbnailStackToNewestOnExpand(
  previousMotion: string | undefined,
  nextMotion: string,
): boolean {
  return nextMotion === "expanded"
    && previousMotion !== undefined
    && previousMotion !== "expanded";
}

/** Scroll offset that puts the newest (bottom) preview in view. */
export function thumbnailStackNewestScrollTop(
  cardCount: number,
  clientHeight: number,
): number {
  return Math.max(0, thumbnailStackContentHeight(cardCount) - Math.max(0, clientHeight));
}

/** Pin the stack to its newest capture using layout geometry, not paint overflow. */
export function scrollThumbnailStackToNewest(stack: HTMLElement): void {
  const cardCount = stack.querySelectorAll(":scope > .thumbnail-card").length;
  stack.scrollTop = thumbnailStackNewestScrollTop(cardCount, stack.clientHeight);
}

export type ScheduleScrollThumbnailStackToNewestOptions = {
  /** Called after each attempt so overflow cues can track the new offset. */
  onScrolled?: () => void;
  /** Injectable rAF. Defaults to `requestAnimationFrame`. */
  frame?: (callback: FrameRequestCallback) => number;
  /** Injectable cancel. Defaults to `cancelAnimationFrame`. */
  cancelFrame?: (id: number) => void;
  /**
   * How long a ResizeObserver may retry after compact→expanded window growth.
   * Omit to only run immediately plus two animation frames.
   */
  retryMs?: number;
};

/**
 * Scroll to the newest capture now and again after layout settles.
 * Compact→expanded and native window growth both change clientHeight a frame
 * later; one useLayoutEffect write is not enough.
 */
export function scheduleScrollThumbnailStackToNewest(
  stack: HTMLElement,
  options: ScheduleScrollThumbnailStackToNewestOptions = {},
): () => void {
  const onScrolled = options.onScrolled;
  const frame = options.frame
    ?? ((callback: FrameRequestCallback) => requestAnimationFrame(callback));
  const cancelFrame = options.cancelFrame
    ?? ((id: number) => cancelAnimationFrame(id));
  const retryMs = options.retryMs ?? 0;

  const run = () => {
    scrollThumbnailStackToNewest(stack);
    onScrolled?.();
  };

  run();
  let innerFrame = 0;
  const outerFrame = frame(() => {
    run();
    innerFrame = frame(run);
  });

  const observer = retryMs > 0 && typeof ResizeObserver === "function"
    ? new ResizeObserver(run)
    : null;
  observer?.observe(stack);
  const timeout = retryMs > 0
    ? window.setTimeout(() => observer?.disconnect(), retryMs)
    : 0;

  return () => {
    cancelFrame(outerFrame);
    cancelFrame(innerFrame);
    observer?.disconnect();
    if (timeout) window.clearTimeout(timeout);
  };
}

export type ThumbnailStackOverflow = {
  /** Older previews are clipped above the visible scrollport. */
  hasOlder: boolean;
  /** Newer previews are clipped below the visible scrollport. */
  hasNewer: boolean;
};

/**
 * Layout height of the stack for `cardCount` cards, matching CSS geometry.
 *
 * Prefer this over `element.scrollHeight` when deciding overflow cues: dust
 * chips and survivor `translateY` paint outside their boxes and can inflate
 * WebKit's scrollable overflow, briefly flashing the "newer" drawer while
 * cards settle into deleted slots.
 */
export function thumbnailStackContentHeight(cardCount: number): number {
  if (cardCount <= 0) return 0;
  return (
    THUMBNAIL_STACK_PADDING_PX
    + THUMBNAIL_STACK_CONTROL_GUTTER_PX
    + cardCount * THUMBNAIL_CARD_HEIGHT_PX
    + (cardCount - 1) * THUMBNAIL_STACK_GAP_PX
  );
}

/**
 * Determine which stack edges have hidden cards. A small tolerance avoids
 * flickering the edge affordances on fractional WebView scroll positions.
 *
 * Pass layout content height (see `thumbnailStackContentHeight`) rather than
 * raw `scrollHeight` so transient paint overflow is ignored.
 */
export function thumbnailStackOverflow(
  scrollTop: number,
  contentHeight: number,
  clientHeight: number,
  tolerance = 1,
): ThumbnailStackOverflow {
  const maxScrollTop = Math.max(0, contentHeight - clientHeight);
  if (maxScrollTop <= tolerance) {
    return { hasOlder: false, hasNewer: false };
  }
  const currentScrollTop = Math.min(maxScrollTop, Math.max(0, scrollTop));
  return {
    hasOlder: currentScrollTop > tolerance,
    hasNewer: currentScrollTop < maxScrollTop - tolerance,
  };
}

/** Ease-out cubic — quick start, soft landing for one-card cue scrolls. */
export function easeOutCubic(t: number): number {
  const x = Math.min(1, Math.max(0, t));
  return 1 - (1 - x) ** 3;
}

export type AnimateThumbnailStackScrollOptions = {
  durationMs?: number;
  reducedMotion?: boolean;
  /** Injectable clock for tests. Defaults to `performance.now`. */
  now?: () => number;
  /** Injectable rAF for tests. Defaults to `requestAnimationFrame`. */
  frame?: (callback: FrameRequestCallback) => number;
  /** Injectable cancel for tests. Defaults to `cancelAnimationFrame`. */
  cancelFrame?: (id: number) => void;
};

/**
 * Animate `stack.scrollTop` toward `targetTop` with ease-out. Returns a cancel
 * function that freezes the scroll at the current interpolated position.
 */
export function animateThumbnailStackScroll(
  stack: HTMLElement,
  targetTop: number,
  options: AnimateThumbnailStackScrollOptions = {},
): () => void {
  const durationMs = options.durationMs ?? THUMBNAIL_STACK_SCROLL_DURATION_MS;
  const reducedMotion = options.reducedMotion ?? false;
  const now = options.now ?? (() => performance.now());
  const frame = options.frame
    ?? ((callback: FrameRequestCallback) => requestAnimationFrame(callback));
  const cancelFrame = options.cancelFrame
    ?? ((id: number) => cancelAnimationFrame(id));

  const startTop = stack.scrollTop;
  const delta = targetTop - startTop;
  if (delta === 0 || reducedMotion || durationMs <= 0) {
    stack.scrollTop = targetTop;
    return () => undefined;
  }

  let cancelled = false;
  let frameId = 0;
  const startTime = now();

  const step = (time: number) => {
    if (cancelled) return;
    const progress = Math.min(1, (time - startTime) / durationMs);
    stack.scrollTop = startTop + delta * easeOutCubic(progress);
    if (progress < 1) {
      frameId = frame(step);
    }
  };
  frameId = frame(step);

  return () => {
    if (cancelled) return;
    cancelled = true;
    cancelFrame(frameId);
  };
}

const STACK_SHIFT_VAR = "--thumbnail-stack-shift";
export const THUMBNAIL_STACK_SHIFTING_CLASS = "thumbnail-stack-shifting";
export const THUMBNAIL_STACK_SHIFT_INSTANT_CLASS = "thumbnail-stack-shift-instant";
const STACK_SHIFTING_CLASS = THUMBNAIL_STACK_SHIFTING_CLASS;
const STACK_SHIFT_INSTANT_CLASS = THUMBNAIL_STACK_SHIFT_INSTANT_CLASS;

/** Classes the stack controller owns; React must preserve them across renders. */
export function thumbnailStackMotionClassNames(card: HTMLElement | null): string[] {
  if (!card) return [];
  return [STACK_SHIFTING_CLASS, STACK_SHIFT_INSTANT_CLASS].filter((name) => (
    card.classList.contains(name)
  ));
}

/**
 * React `className` rewrites drop controller tokens. If a stacked offset is
 * still applied, put the shifting class back before paint so the card cannot
 * flash at its untranslated layout slot.
 */
export function restoreThumbnailStackShiftClass(card: HTMLElement | null): void {
  if (!card) return;
  const shiftPx = Number.parseFloat(card.style.getPropertyValue(STACK_SHIFT_VAR).trim());
  if (Number.isFinite(shiftPx) && shiftPx > 0) {
    card.classList.add(STACK_SHIFTING_CLASS);
  }
}

/**
 * Visual translateY currently on `card`, in CSS pixels.
 * Prefers the computed matrix so a mid-ease freeze matches what the user sees.
 */
export function readComputedTranslateY(card: HTMLElement): number | null {
  if (typeof getComputedStyle !== "function") return null;
  try {
    const style = getComputedStyle(card);
    const transform = style.transform;
    if (transform && transform !== "none") {
      const matrix = new DOMMatrixReadOnly(transform);
      if (Number.isFinite(matrix.f)) return matrix.f;
    }
    const translate = style.translate;
    if (translate && translate !== "none") {
      const parts = translate.trim().split(/\s+/);
      const yToken = parts.length >= 2 ? parts[1] : "0";
      const parsed = Number.parseFloat(yToken);
      if (Number.isFinite(parsed)) return parsed;
    }
  } catch {
    return null;
  }
  return null;
}

function isDustDeleteCard(card: HTMLElement): boolean {
  return card.classList.contains("thumbnail-exit-delete")
    && card.classList.contains("thumbnail-exit-dust");
}

function isDismissCard(card: HTMLElement): boolean {
  return card.classList.contains("thumbnail-exit-dismiss");
}

/** Exits that hold layout and drive the shared survivor settle. */
function isHeldLayoutExitCard(card: HTMLElement): boolean {
  return isDismissCard(card) || isDustDeleteCard(card);
}

function motionDelayMsFor(card: HTMLElement): number {
  if (isDismissCard(card)) return THUMBNAIL_DISMISS_STACK_MOTION_DELAY_MS;
  return THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS;
}

function isExitingCard(card: HTMLElement): boolean {
  return card.classList.contains("thumbnail-exiting");
}

function readStackShiftPx(card: HTMLElement): number {
  const raw = card.style.getPropertyValue(STACK_SHIFT_VAR).trim();
  if (!raw) return 0;
  const parsed = Number.parseFloat(raw);
  return Number.isFinite(parsed) ? parsed : 0;
}

function writeTranslatePx(card: HTMLElement, shiftPx: number): void {
  card.style.setProperty(STACK_SHIFT_VAR, `${shiftPx}px`);
  // Inline `translate` survives React className rewrites that drop the shifting
  // class for a frame; it also composes with dismiss `transform: translateX`.
  card.style.setProperty("translate", `0 ${shiftPx}px`);
}

function clearTranslatePx(card: HTMLElement): void {
  card.style.removeProperty(STACK_SHIFT_VAR);
  card.style.removeProperty("translate");
}

function writeStackShiftPx(card: HTMLElement, shiftPx: number, animate: boolean): void {
  if (shiftPx <= 0) {
    const hadVisualShift = card.classList.contains(STACK_SHIFTING_CLASS)
      || readStackShiftPx(card) > 0
      || Boolean(card.style.translate);
    if (hadVisualShift) {
      // Snap with layout reflow. An ease here would slide the card back up
      // while a hole below is collapsing.
      card.classList.add(STACK_SHIFT_INSTANT_CLASS);
      card.classList.remove(STACK_SHIFTING_CLASS);
      clearTranslatePx(card);
      void card.offsetWidth;
      card.classList.remove(STACK_SHIFT_INSTANT_CLASS);
      return;
    }
    card.classList.remove(STACK_SHIFTING_CLASS);
    card.classList.remove(STACK_SHIFT_INSTANT_CLASS);
    clearTranslatePx(card);
    return;
  }

  if (!animate) {
    card.classList.add(STACK_SHIFT_INSTANT_CLASS);
    card.classList.add(STACK_SHIFTING_CLASS);
    writeTranslatePx(card, shiftPx);
    // Force the browser to commit the snapped value before re-enabling easing
    // so a later increase still transitions from the correct origin.
    void card.offsetWidth;
    card.classList.remove(STACK_SHIFT_INSTANT_CLASS);
    return;
  }

  card.classList.remove(STACK_SHIFT_INSTANT_CLASS);
  card.classList.add(STACK_SHIFTING_CLASS);
  writeTranslatePx(card, shiftPx);
}

type StackTransition = Animation & {
  transitionProperty?: string;
};

function activeThumbnailStackTransitions(stack: HTMLElement): Animation[] {
  const transitions: Animation[] = [];
  const survivors = stack.querySelectorAll<HTMLElement>(
    ":scope > .thumbnail-card.thumbnail-stack-shifting:not(.thumbnail-exiting)",
  );
  for (const survivor of survivors) {
    if (typeof survivor.getAnimations !== "function") continue;
    let animations: Animation[];
    try {
      animations = survivor.getAnimations();
    } catch {
      continue;
    }
    for (const animation of animations) {
      const transition = animation as StackTransition;
      if (!transition.transitionProperty) continue;
      if (!transition.pending && transition.playState !== "running") continue;
      transitions.push(animation);
    }
  }
  return transitions;
}

function waitForAnimationBatch(
  animations: readonly Animation[],
  timeoutMs: number,
): Promise<void> {
  return new Promise((resolve) => {
    let complete = false;
    const finish = () => {
      if (complete) return;
      complete = true;
      clearTimeout(timeout);
      resolve();
    };
    const timeout = setTimeout(finish, Math.max(0, timeoutMs));
    void Promise.allSettled(animations.map((animation) => animation.finished))
      .then(finish);
  });
}

/**
 * Wait until the browser has actually finished moving survivors before an
 * exiting card releases its held flex slot. Delete and dismiss timings can
 * overlap and retarget the same CSS transition; relying on either exit's
 * nominal duration can otherwise remove a slot mid-transition and snap the
 * remaining stack to its stored target.
 */
export async function waitForThumbnailStackSettle(
  exitCard: HTMLElement | null,
  maxWaitMs = THUMBNAIL_STACK_SETTLE_MAX_WAIT_MS,
): Promise<void> {
  const stack = exitCard?.parentElement;
  if (!stack) return;
  const deadline = Date.now() + Math.max(0, maxWaitMs);

  while (exitCard.isConnected) {
    const transitions = activeThumbnailStackTransitions(stack);
    if (transitions.length === 0) return;
    const remainingMs = deadline - Date.now();
    if (remainingMs <= 0) return;
    // A transition's `finished` promise rejects when a later exit retargets
    // it. Re-query after every batch so the replacement transition is awaited.
    await waitForAnimationBatch(transitions, remainingMs);
  }
}

/**
 * Drive multi-slot stack collapse for held-layout exits (dust delete + dismiss).
 *
 * Survivors slide by N × slot with the same ease for both exit kinds. When a
 * finished exit is removed, the transform snaps down with the layout reflow
 * so multi-exit batches do not teleport.
 */
export function createThumbnailStackShiftController(stack: HTMLElement): () => void {
  const exitStartedAt = new WeakMap<HTMLElement, number>();
  const scheduledTimers = new Set<ReturnType<typeof setTimeout>>();
  let microtaskQueued = false;

  const clearTimers = () => {
    for (const timer of scheduledTimers) clearTimeout(timer);
    scheduledTimers.clear();
  };

  const schedule = (delayMs: number) => {
    const timer = setTimeout(() => {
      scheduledTimers.delete(timer);
      applyShifts();
    }, Math.max(0, delayMs));
    scheduledTimers.add(timer);
  };

  const applyShifts = () => {
    const cards = Array.from(
      stack.querySelectorAll<HTMLElement>(":scope > .thumbnail-card"),
    );
    const now = performance.now();

    for (const card of cards) {
      if (!isHeldLayoutExitCard(card)) continue;
      if (exitStartedAt.has(card)) continue;
      exitStartedAt.set(card, now);
      // Wake once this slot becomes motion-ready (plus a frame of slack).
      schedule(motionDelayMsFor(card) + 16);
    }

    const motionStates: ThumbnailStackCardMotionState[] = cards.map((card) => {
      const holdsLayoutSlot = isHeldLayoutExitCard(card);
      const startedAt = exitStartedAt.get(card);
      const delayMs = motionDelayMsFor(card);
      const motionReady = holdsLayoutSlot
        && startedAt !== undefined
        && now - startedAt >= delayMs;
      const exiting = isExitingCard(card);
      let currentShiftPx = readStackShiftPx(card);
      if (exiting && currentShiftPx > 0) {
        // Freeze mid-ease so delete/dismiss starts where the card actually is,
        // not at the still-animating target slot. Ignore a 0/identity matrix —
        // jsdom and some WebViews report no visual translate even while the
        // CSS variable still holds the stacked offset.
        const visualPx = readComputedTranslateY(card);
        if (visualPx !== null && visualPx > 0.5 && Math.abs(visualPx - currentShiftPx) > 0.5) {
          writeStackShiftPx(card, visualPx, false);
          currentShiftPx = visualPx;
        }
      }
      return {
        exiting,
        holdsLayoutSlot,
        motionReady,
        currentShiftPx,
      };
    });

    const shifts = computeThumbnailStackShifts(motionStates);
    for (let index = 0; index < cards.length; index += 1) {
      const card = cards[index]!;
      const nextPx = shifts[index] ?? 0;
      const previousPx = readStackShiftPx(card);
      if (previousPx === nextPx) {
        // WebKit emits a class-attribute MutationRecord even when classList.add
        // repeats an existing token. Since this controller observes `class`,
        // rewriting the settled class would queue applyShifts forever and
        // starve timers, hover polling, clicks, and exit completion.
        if (nextPx > 0 && !card.classList.contains(STACK_SHIFTING_CLASS)) {
          card.classList.add(STACK_SHIFTING_CLASS);
        }
        continue;
      }
      writeStackShiftPx(
        card,
        nextPx,
        shouldAnimateThumbnailStackShift(previousPx, nextPx),
      );
    }
  };

  // Coalesce to one apply per turn, but stay before paint so a finished exit's
  // layout reflow and transform drop land in the same frame (no teleport flash).
  const queueApply = () => {
    if (microtaskQueued) return;
    microtaskQueued = true;
    queueMicrotask(() => {
      microtaskQueued = false;
      applyShifts();
    });
  };

  const observer = new MutationObserver(queueApply);
  observer.observe(stack, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["class"],
  });

  // Initial sync in case exits already started before the controller bound.
  queueApply();

  return () => {
    observer.disconnect();
    clearTimers();
    for (const card of stack.querySelectorAll<HTMLElement>(":scope > .thumbnail-card")) {
      card.classList.remove(STACK_SHIFTING_CLASS);
      card.classList.remove(STACK_SHIFT_INSTANT_CLASS);
      clearTranslatePx(card);
    }
  };
}
