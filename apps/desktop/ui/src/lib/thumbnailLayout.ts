/** Thumbnail card height in CSS pixels (matches `.thumbnail-card` flex-basis/height). */
export const THUMBNAIL_CARD_HEIGHT_PX = 160;

/** Vertical gap between cards (matches `.thumbnail-stack` gap). */
export const THUMBNAIL_STACK_GAP_PX = 24;

/** Top/side padding on `.thumbnail-stack`. */
export const THUMBNAIL_STACK_PADDING_PX = 28;

/** Expanded Show less gutter (bottom padding, swapped to the top when top-anchored). */
export const THUMBNAIL_STACK_CONTROL_GUTTER_PX = 52;

/** One stack slot: card height + inter-card gap. */
export const THUMBNAIL_CARD_SLOT_PX = THUMBNAIL_CARD_HEIGHT_PX + THUMBNAIL_STACK_GAP_PX;

/**
 * Typical small pile used as the default hover-fan stagger bound.
 * Fade/blur still reads around this depth; peek spacing no longer cliffs here.
 */
export const THUMBNAIL_STACK_FULL_PEEK_DEPTH = 3;

/**
 * Far-field peek step as a fraction of a full 9px idle step.
 * Deep cards pack toward this so a long history recedes instead of hiding.
 * Mirrored in mini-preview.css as `--thumbnail-stack-recede`.
 */
export const THUMBNAIL_STACK_RECEDING_STEP = 0.55;

/**
 * Softens receding so consecutive peeks stay close. Larger = more uniform
 * gaps through the front of the pile. Mirrored in mini-preview.css as
 * `--thumbnail-stack-pose-k`.
 */
export const THUMBNAIL_STACK_POSE_EASE_K = 24;

/** Max idle peek nudge on the first behind-card (px). Deeper cards damp toward 0. */
export const THUMBNAIL_STACK_PEEK_JITTER_PX = 0.4;

/** How quickly peek jitter settles toward the back of the pile. */
export const THUMBNAIL_STACK_PEEK_JITTER_DECAY = 0.58;

/** Idle collapsed peek per pose unit (matches compact rest `translateY`). */
export const THUMBNAIL_STACK_IDLE_PEEK_PX = 9;

/** Hover-fan collapsed peek per pose unit (matches compact hover `translateY`). */
export const THUMBNAIL_STACK_HOVER_PEEK_PX = 11;

/** Extra delay per stacked card so collapsed hover lift does not fire in lockstep. */
export const THUMBNAIL_STACK_FAN_STAGGER_MS = 16;

/**
 * Hover fan and press-gather duration. Matches `--dur-3` / `--stack-fan-dur`
 * so the scattered pile eases back to rest instead of snapping on click.
 */
export const THUMBNAIL_STACK_FAN_DURATION_MS = 200;

/**
 * Time until the deepest card has finished gathering from the hover fan.
 * Drag sway waits for this so it does not snap onto a mid-ease pose.
 * Matches CSS `pile-depth * --stack-fan-stagger` on the press transition.
 */
export function thumbnailStackFanCollapseMs(
  cardCount = THUMBNAIL_STACK_FULL_PEEK_DEPTH + 1,
): number {
  const extra = Math.max(cardCount - 1, 0);
  return (
    THUMBNAIL_STACK_FAN_DURATION_MS
    + thumbnailStackPoseDepth(extra) * THUMBNAIL_STACK_FAN_STAGGER_MS
  );
}

/**
 * Per-capture rest scatter. Seeded from the artifact id so a capture keeps
 * the same lean as newer shots push it back, instead of snapping onto a
 * depth-indexed slot that looks identical every time. The front card stays
 * square; behind-cards stay within about a degree and a couple of pixels.
 */
export const THUMBNAIL_STACK_REST_SKEW_X_PX = 3.5;
export const THUMBNAIL_STACK_REST_SKEW_Y_PX = 1.5;
export const THUMBNAIL_STACK_REST_SKEW_ROT_DEG = 1.5;
export const THUMBNAIL_STACK_HOVER_SKEW_X_PX = 4.5;
export const THUMBNAIL_STACK_HOVER_SKEW_ROT_DEG = 2;

/** Keep behind-cards off the dead zone so a new capture never looks slotted. */
export const THUMBNAIL_STACK_SKEW_MIN_UNIT = 0.22;

/** Compact card width: window 340 minus 28px padding on each side. */
export const THUMBNAIL_STACK_CARD_WIDTH_PX = 284;

/**
 * How far a bottom-centered rotateZ swings the top corners past the unrotated
 * card box. CSS `rotateZ` is clockwise.
 */
export function thumbnailStackSkewCornerExtra(rotateDeg: number): { x: number; y: number } {
  const halfWidth = THUMBNAIL_STACK_CARD_WIDTH_PX / 2;
  let extraX = 0;
  let extraY = 0;
  for (const sign of [-1, 1]) {
    const radians = (sign * Math.abs(rotateDeg) * Math.PI) / 180;
    const cos = Math.cos(radians);
    const sin = Math.sin(radians);
    for (const [x, y] of [
      [-halfWidth, -THUMBNAIL_CARD_HEIGHT_PX],
      [halfWidth, -THUMBNAIL_CARD_HEIGHT_PX],
    ] as const) {
      const rotatedX = x * cos + y * sin;
      const rotatedY = -x * sin + y * cos;
      extraX = Math.max(extraX, Math.abs(rotatedX) - halfWidth);
      extraY = Math.max(extraY, -rotatedY - THUMBNAIL_CARD_HEIGHT_PX);
    }
  }
  return { x: extraX, y: extraY };
}

/** Worst-case paint overflow from unique skew plus that rotation. */
export function thumbnailStackSkewPaintOverflow(hovered: boolean): { x: number; y: number } {
  const rotate = hovered
    ? THUMBNAIL_STACK_HOVER_SKEW_ROT_DEG
    : THUMBNAIL_STACK_REST_SKEW_ROT_DEG;
  const shiftX = hovered
    ? THUMBNAIL_STACK_HOVER_SKEW_X_PX
    : THUMBNAIL_STACK_REST_SKEW_X_PX;
  const corner = thumbnailStackSkewCornerExtra(rotate);
  return {
    x: corner.x + shiftX,
    y: corner.y + THUMBNAIL_STACK_REST_SKEW_Y_PX,
  };
}

/** Extra idle peek so rest rotation and Y jitter stay inside the hit target. */
export const THUMBNAIL_STACK_REST_SKEW_PEEK_PX = Math.ceil(
  thumbnailStackSkewPaintOverflow(false).y,
);

/** Extra hit-target height so a hovered tilt corner is still on the pile. */
export const THUMBNAIL_STACK_HOVER_TILT_PEEK_PX = Math.ceil(
  thumbnailStackSkewPaintOverflow(true).y,
);

/**
 * Horizontal inset to pull back so scattered rear-card edges stay on the
 * expand control. Capped at side padding so a single square preview does
 * not steal the click-through gutters; `thumbnailStackSkewHitPadPx` zeros
 * that pad for a one-card pile.
 */
export const THUMBNAIL_STACK_SKEW_HIT_PAD_PX = Math.min(
  THUMBNAIL_STACK_PADDING_PX,
  Math.ceil(thumbnailStackSkewPaintOverflow(true).x),
);

/** Collapsed expand-control inset for `cardCount` previews. */
export function thumbnailStackSkewHitPadPx(cardCount: number): number {
  return cardCount > 1 ? THUMBNAIL_STACK_SKEW_HIT_PAD_PX : 0;
}

export type ThumbnailStackSkew = {
  restX: number;
  restY: number;
  restRotate: number;
  fanShift: number;
  fanTilt: number;
};

const ZERO_THUMBNAIL_STACK_SKEW: ThumbnailStackSkew = {
  restX: 0,
  restY: 0,
  restRotate: 0,
  fanShift: 0,
  fanTilt: 0,
};

/** FNV-1a with an extra avalanche mix, mapped away from zero onto [-1, -min] ∪ [min, 1]. */
function thumbnailStackSkewUnit(id: string, salt: number): number {
  let hash = (2166136261 ^ salt) >>> 0;
  for (let index = 0; index < id.length; index += 1) {
    hash ^= id.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  hash = Math.imul(hash ^ (hash >>> 16), 2246822519);
  hash = Math.imul(hash ^ (hash >>> 13), 3266489917);
  hash ^= hash >>> 16;
  const unit = (hash >>> 0) / 0xffffffff;
  const side = unit < 0.5 ? -1 : 1;
  const span = 1 - THUMBNAIL_STACK_SKEW_MIN_UNIT;
  const magnitude = THUMBNAIL_STACK_SKEW_MIN_UNIT + (unit % 0.5) * 2 * span;
  return side * Math.min(magnitude, 1);
}

function cssQty(value: number, unit: string): string {
  return `${Number(value.toFixed(3))}${unit}`;
}

/** Unique collapsed lean for `id` at `depth`. Depth 0 is always square. */
export function thumbnailStackSkew(id: string, depth: number): ThumbnailStackSkew {
  if (depth <= 0 || !id) return ZERO_THUMBNAIL_STACK_SKEW;
  const x = thumbnailStackSkewUnit(id, 0x51ed);
  const y = thumbnailStackSkewUnit(id, 0x9e37);
  const rotate = thumbnailStackSkewUnit(id, 0xc2b2);
  return {
    restX: x * THUMBNAIL_STACK_REST_SKEW_X_PX,
    restY: y * THUMBNAIL_STACK_REST_SKEW_Y_PX,
    restRotate: rotate * THUMBNAIL_STACK_REST_SKEW_ROT_DEG,
    fanShift: x * THUMBNAIL_STACK_HOVER_SKEW_X_PX,
    fanTilt: rotate * THUMBNAIL_STACK_HOVER_SKEW_ROT_DEG,
  };
}

/** Inline CSS variables for the compact pile pose. */
export function thumbnailStackSkewCssVars(
  id: string,
  depth: number,
): Record<string, string | number> {
  const skew = thumbnailStackSkew(id, depth);
  return {
    "--thumbnail-stack-base-depth": depth,
    "--thumbnail-stack-skew-x": cssQty(skew.restX, "px"),
    "--thumbnail-stack-skew-y": cssQty(skew.restY, "px"),
    "--thumbnail-stack-skew-rot": cssQty(skew.restRotate, "deg"),
    "--thumbnail-stack-fan-tilt": cssQty(skew.fanTilt, "deg"),
    "--thumbnail-stack-fan-shift": cssQty(skew.fanShift, "px"),
  };
}

/**
 * Visual collapsed depth. Consecutive peeks ease from a full step toward
 * {@link THUMBNAIL_STACK_RECEDING_STEP} so the pile recedes without a gap
 * jump where fade/blur picks up. Mirrored in mini-preview.css as
 * `--thumbnail-stack-pose` / `--thumbnail-stack-pile-depth`.
 */
export function thumbnailStackPoseDepth(depth: number): number {
  const n = Math.max(0, depth);
  if (n === 0) return 0;
  return (
    n
    * (THUMBNAIL_STACK_POSE_EASE_K + THUMBNAIL_STACK_RECEDING_STEP * n)
    / (n + THUMBNAIL_STACK_POSE_EASE_K)
  );
}

/** Peek step from `depth - 1` to `depth`, in pose units. */
export function thumbnailStackPoseStep(depth: number): number {
  const n = Math.max(0, depth);
  if (n <= 0) return 0;
  return thumbnailStackPoseDepth(n) - thumbnailStackPoseDepth(n - 1);
}

/**
 * Tiny signed rest-pose nudge so stacked peeks are not a perfectly even
 * ruler. Amplitude falls off with depth so the faded tail stays even.
 */
export function thumbnailStackPeekJitterPx(depth: number): number {
  const n = Math.max(0, Math.trunc(depth));
  if (n <= 0) return 0;
  return (
    thumbnailStackPeekJitterUnit(n)
    * THUMBNAIL_STACK_PEEK_JITTER_PX
    * THUMBNAIL_STACK_PEEK_JITTER_DECAY ** (n - 1)
  );
}

/** Deterministic signed unit in (-1, 1) from depth so every pile matches. */
function thumbnailStackPeekJitterUnit(depth: number): number {
  const hashed = Math.imul(depth * 0x9e3779b1 ^ 0x7f4a7c15, 0x85ebca6b) >>> 0;
  return hashed / 2 ** 32 * 2 - 1;
}

/** Compact rest/hover depth. Same as pose: extras recede instead of clamping. */
export function thumbnailStackPileDepth(depth: number): number {
  return thumbnailStackPoseDepth(depth);
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
  const extra = Math.max(cardCount - 1, 0);
  const pose = thumbnailStackPoseDepth(extra);
  const peek = pose * (hovered ? THUMBNAIL_STACK_HOVER_PEEK_PX : THUMBNAIL_STACK_IDLE_PEEK_PX);
  if (extra <= 0) return peek;
  return peek + (hovered
    ? THUMBNAIL_STACK_HOVER_TILT_PEEK_PX
    : THUMBNAIL_STACK_REST_SKEW_PEEK_PX);
}

/**
 * Collapsed native/harness frame height matching `thumbnail_stack_height`
 * in Rust: front card + gutters + hover peek that sticks out of padding.
 */
export function thumbnailCollapsedFrameHeight(cardCount: number): number {
  const peek = thumbnailCollapsedPeekPx(Math.max(cardCount, 1), true);
  const extraAbovePadding = Math.max(0, peek - THUMBNAIL_STACK_PADDING_PX);
  return (
    THUMBNAIL_STACK_PADDING_PX
    + THUMBNAIL_STACK_CONTROL_GUTTER_PX
    + THUMBNAIL_CARD_HEIGHT_PX
    + extraAbovePadding
  );
}

/**
 * Signed pile gravity. +1 = bottom of the screen (peek up), 0 = middle
 * (structured peek tucked; per-capture skew remains), -1 = top (peek down).
 */
export type ThumbnailStackAnchor = "top" | "bottom";

export const THUMBNAIL_STACK_GRAVITY_VAR = "--thumbnail-stack-gravity";

/** Switch to a top pile once gravity is clearly in the upper band. */
export const THUMBNAIL_STACK_ANCHOR_TOP_GRAVITY = -0.2;

/** Switch back to a bottom pile once gravity is clearly in the lower band. */
export const THUMBNAIL_STACK_ANCHOR_BOTTOM_GRAVITY = 0.2;

function clampGravity(value: number): number {
  if (!Number.isFinite(value)) return 1;
  return Math.min(1, Math.max(-1, value));
}

/**
 * Map a 0 (top) … 1 (bottom) travel through the work area onto signed gravity.
 */
export function thumbnailStackGravityFromNormalizedY(yFromTop: number): number {
  if (!Number.isFinite(yFromTop)) return 1;
  return clampGravity(2 * yFromTop - 1);
}

export type ThumbnailStackHarnessGravity = {
  offsetY: number;
  anchor: ThumbnailStackAnchor;
  viewportHeight: number;
  contentHeight: number;
};

/** Visible pile top in CSS pixels for a harness `#root` translation. */
export function thumbnailStackHarnessPileTop({
  offsetY,
  anchor,
  viewportHeight,
  contentHeight,
}: ThumbnailStackHarnessGravity): number {
  const content = Math.max(0, contentHeight);
  if (anchor === "top") return offsetY;
  return viewportHeight + offsetY - content;
}

export function thumbnailStackGravityFromHarness(
  options: ThumbnailStackHarnessGravity,
): number {
  const travel = Math.max(1, options.viewportHeight - Math.max(0, options.contentHeight));
  return thumbnailStackGravityFromNormalizedY(
    thumbnailStackHarnessPileTop(options) / travel,
  );
}

export function thumbnailStackAnchorFromGravity(
  gravity: number,
  current: ThumbnailStackAnchor = "bottom",
): ThumbnailStackAnchor {
  if (current === "bottom" && gravity <= THUMBNAIL_STACK_ANCHOR_TOP_GRAVITY) {
    return "top";
  }
  if (current === "top" && gravity >= THUMBNAIL_STACK_ANCHOR_BOTTOM_GRAVITY) {
    return "bottom";
  }
  return current;
}

/** Convert a harness `#root` translation between top and bottom anchoring. */
export function convertHarnessStackOffsetAnchor(
  offset: { x: number; y: number },
  from: ThumbnailStackAnchor,
  to: ThumbnailStackAnchor,
  viewportHeight: number,
  contentHeight: number,
): { x: number; y: number } {
  if (from === to) return offset;
  const pileTop = thumbnailStackHarnessPileTop({
    offsetY: offset.y,
    anchor: from,
    viewportHeight,
    contentHeight,
  });
  if (to === "top") return { x: offset.x, y: pileTop };
  return { x: offset.x, y: pileTop + contentHeight - viewportHeight };
}

/**
 * Visible pile bottom in screen space. Bottom-aligned native windows keep
 * empty chrome above the pile; top-aligned ones keep it below so peek-down
 * is not clipped by the webview.
 */
export function thumbnailStackVisualPileBottom({
  y,
  frameHeight,
  contentHeight,
  anchor,
}: {
  y: number;
  frameHeight: number;
  contentHeight: number;
  anchor: ThumbnailStackAnchor;
}): number {
  const frame = Math.max(0, frameHeight);
  const content = Math.min(frame, Math.max(0, contentHeight));
  return anchor === "top" ? y + content : y + frame;
}

/** Keep the visible pile in place when the native frame flips its slack. */
export function convertThumbnailStackFrameAnchor(
  offset: { x: number; y: number },
  from: ThumbnailStackAnchor,
  to: ThumbnailStackAnchor,
  frameHeight: number,
  contentHeight: number,
): { x: number; y: number } {
  if (from === to) return offset;
  const frame = Math.max(0, frameHeight);
  const content = Math.min(frame, Math.max(0, contentHeight));
  const slack = Math.max(0, frame - content);
  const pileTop = from === "top" ? offset.y : offset.y + slack;
  return { x: offset.x, y: to === "top" ? pileTop : pileTop - slack };
}

export function applyThumbnailStackGravity(
  stack: HTMLElement | null,
  gravity: number,
) {
  if (!stack) return;
  stack.style.setProperty(
    THUMBNAIL_STACK_GRAVITY_VAR,
    String(Number(clampGravity(gravity).toFixed(4))),
  );
}

export type ThumbnailStackWorkGravity = {
  pileBottom: number;
  workTop: number;
  workHeight: number;
  contentHeight: number;
  bottomGap?: number;
};

/** Gravity from a native window's visible pile bottom in the monitor work area. */
export function thumbnailStackGravityFromWorkArea({
  pileBottom,
  workTop,
  workHeight,
  contentHeight,
  bottomGap = 0,
}: ThumbnailStackWorkGravity): number {
  const workBottom = workTop + workHeight - Math.max(0, bottomGap);
  const travel = workBottom - workTop - Math.max(0, contentHeight);
  if (travel <= 1) return 1;
  const fromTop = (workBottom - pileBottom) / travel;
  return thumbnailStackGravityFromNormalizedY(1 - fromTop);
}

/** Duration for one-slot overflow-cue scrolls (ease-out). */
export const THUMBNAIL_STACK_SCROLL_DURATION_MS = 380;

/**
 * Expand/collapse fly between the list and the compact pile.
 * Matches `thumbnail-card-expand` / minimize-run CSS (0.52s).
 */
export const THUMBNAIL_STACK_EXPAND_COLLAPSE_MS = 520;

/** Hover morph: stack icon width → Show less pill (and the reverse). */
export const THUMBNAIL_MINIMIZE_MORPH_MS = 240;

/** Hover morph: stack icon / Show less label crossfade. */
export const THUMBNAIL_MINIMIZE_SWAP_MS = 180;

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
 * Expanding the pile should land on the newest capture. Compact cards are
 * absolutely positioned, so the in-flow list starts at scrollTop 0 (oldest)
 * unless we pin before the first expanded paint.
 */
export function shouldScrollThumbnailStackToNewestOnExpand(
  previousMotion: string | undefined,
  nextMotion: string,
): boolean {
  return nextMotion === "expanded"
    && previousMotion !== undefined
    && previousMotion !== "expanded";
}

/**
 * Bounded window-filling scrollport. Compact uses height 100%; expanded uses
 * height auto + max-height 100%, which can stay content-sized for a frame and
 * clamp newest-scroll to 0. Keep this class through expand so the first
 * in-flow layout can actually overflow.
 */
export const THUMBNAIL_STACK_SCROLLPORT_CLASS = "thumbnail-stack-scrollport";

/** Containing-block height for the stack, falling back to the WebView. */
export function thumbnailStackViewportHeight(stack: HTMLElement): number {
  const containing = stack.parentElement;
  const containingHeight = containing?.clientHeight ?? 0;
  if (containingHeight > 0) return containingHeight;
  return typeof window !== "undefined" ? window.innerHeight : 0;
}

/** True when the expanded list is taller than the visible window. */
export function thumbnailStackNeedsScrollport(
  cardCount: number,
  viewportHeight: number,
): boolean {
  return thumbnailStackContentHeight(cardCount) - Math.max(0, viewportHeight) > 1;
}

/** Scroll offset that puts the newest preview in view. */
export function thumbnailStackNewestScrollTop(
  cardCount: number,
  clientHeight: number,
  fromTop = false,
): number {
  if (fromTop) return 0;
  return Math.max(0, thumbnailStackContentHeight(cardCount) - Math.max(0, clientHeight));
}

export type ScrollThumbnailStackToNewestOptions = {
  /** Visible window height. Defaults to the stack's containing block. */
  viewportHeight?: number;
  /** Newest capture is at the start of the list (top-anchored expand). */
  fromTop?: boolean;
};

/**
 * Pin the stack to its newest capture using layout geometry, not paint overflow.
 * If the stack is still content-sized, fill the viewport first so `scrollTop`
 * can land in this layout pass instead of waiting for a later frame.
 */
export function scrollThumbnailStackToNewest(
  stack: HTMLElement,
  options: ScrollThumbnailStackToNewestOptions = {},
): void {
  const cardCount = stack.querySelectorAll(":scope > .thumbnail-card").length;
  const contentHeight = thumbnailStackContentHeight(cardCount);
  const viewportHeight = options.viewportHeight ?? thumbnailStackViewportHeight(stack);
  if (thumbnailStackNeedsScrollport(cardCount, viewportHeight)) {
    stack.classList.add(THUMBNAIL_STACK_SCROLLPORT_CLASS);
  } else {
    stack.classList.remove(THUMBNAIL_STACK_SCROLLPORT_CLASS);
  }
  const measured = stack.clientHeight;
  const scrollPortHeight = measured > 0 && measured < contentHeight - 1
    ? measured
    : (thumbnailStackNeedsScrollport(cardCount, viewportHeight) ? viewportHeight : measured);
  stack.scrollTop = thumbnailStackNewestScrollTop(
    cardCount,
    scrollPortHeight,
    options.fromTop,
  );
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
  /** Visible window height forwarded to each pin attempt. */
  viewportHeight?: number;
  /** Newest capture is at the start of the list (top-anchored expand). */
  fromTop?: boolean;
};

/**
 * Scroll to the newest capture now and again after layout settles.
 * Native window growth can still change clientHeight a frame later; pinning
 * with a viewport fill covers the compact→expanded handover in this pass.
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
  const viewportHeight = options.viewportHeight;

  const run = () => {
    scrollThumbnailStackToNewest(stack, {
      viewportHeight,
      fromTop: options.fromTop,
    });
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
const STACK_SHIFT_SLOTS_VAR = "--thumbnail-stack-shift-slots";
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

/** Compact / collapsed / expanding piles pose with `transform`, not slot `translate`. */
export function thumbnailStackSuppressesSlotShift(stack: HTMLElement): boolean {
  return stack.classList.contains("thumbnail-stack-compact");
}

/** How many expanded slots `shiftPx` represents, for compact visual depth. */
export function thumbnailStackShiftSlots(shiftPx: number): number {
  return Math.max(0, shiftPx) / THUMBNAIL_CARD_SLOT_PX;
}

function writeShiftSlots(card: HTMLElement, slots: number): void {
  if (slots <= 0) {
    card.style.removeProperty(STACK_SHIFT_SLOTS_VAR);
    return;
  }
  card.style.setProperty(STACK_SHIFT_SLOTS_VAR, String(slots));
}

function clearShiftSlots(card: HTMLElement): void {
  card.style.removeProperty(STACK_SHIFT_SLOTS_VAR);
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

    if (thumbnailStackSuppressesSlotShift(stack)) {
      // Compact pose is a 3D `transform` from a shared bottom anchor. Expanded
      // slot `translate` would compose with that and drop survivors below the
      // front card until the held exit is removed. Snapshot any in-flight
      // settle as compact depth so Show less does not jump cards back up to
      // their original expanded slots.
      for (const card of cards) {
        const shiftPx = readStackShiftPx(card);
        if (shiftPx > 0) writeShiftSlots(card, thumbnailStackShiftSlots(shiftPx));
        const hasSlotShift = card.classList.contains(STACK_SHIFTING_CLASS)
          || card.classList.contains(STACK_SHIFT_INSTANT_CLASS)
          || shiftPx > 0
          || Boolean(card.style.translate);
        if (hasSlotShift) writeStackShiftPx(card, 0, false);
      }
      return;
    }

    for (const card of cards) clearShiftSlots(card);

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
      clearShiftSlots(card);
    }
  };
}
