/** Thumbnail card height in CSS pixels (matches `.thumbnail-card` flex-basis/height). */
export const THUMBNAIL_CARD_HEIGHT_PX = 160;

/** Vertical gap between cards (matches `.thumbnail-stack` gap). */
export const THUMBNAIL_STACK_GAP_PX = 24;

/** One stack slot: card height + inter-card gap. */
export const THUMBNAIL_CARD_SLOT_PX = THUMBNAIL_CARD_HEIGHT_PX + THUMBNAIL_STACK_GAP_PX;

/**
 * Delay before live cards above a dust-delete begin sliding into the empty
 * slot. Matches the pre-motion ash phase in styles.css.
 */
export const THUMBNAIL_STACK_MOTION_DELAY_MS = 1_800;

/** Duration of the stack settle slide (matches prior CSS animation). */
export const THUMBNAIL_STACK_MOTION_DURATION_MS = 580;

export type ThumbnailStackCardMotionState = {
  /** True while this card is locked in any exit animation. */
  exiting: boolean;
  /**
   * True when this card is a dust-delete exit that still occupies layout
   * space and should eventually pull cards above it downward.
   */
  deleteDust: boolean;
  /**
   * True once this delete's motion delay has elapsed so its slot contributes
   * to the stacked shift of live cards above it.
   */
  motionReady: boolean;
};

/**
 * Count how many motion-ready dust-delete slots sit below `index` in the
 * bottom-anchored stack. Live cards slide by this many slots.
 */
export function countMotionReadyDeleteSlotsBelow(
  cards: readonly ThumbnailStackCardMotionState[],
  index: number,
): number {
  let count = 0;
  for (let i = index + 1; i < cards.length; i += 1) {
    const card = cards[i];
    if (card?.deleteDust && card.motionReady) count += 1;
  }
  return count;
}

/** Pixel shift for a live card sitting above `slots` open delete holes. */
export function thumbnailStackShiftPx(slots: number): number {
  return Math.max(0, slots) * THUMBNAIL_CARD_SLOT_PX;
}

/**
 * Compute the target translateY (px) for every card in order.
 * Exiting cards never carry a shift — only live survivors slide.
 */
export function computeThumbnailStackShifts(
  cards: readonly ThumbnailStackCardMotionState[],
): number[] {
  return cards.map((card, index) => {
    if (card.exiting) return 0;
    return thumbnailStackShiftPx(countMotionReadyDeleteSlotsBelow(cards, index));
  });
}

/**
 * Increases should ease so multi-delete stacks accumulate smoothly.
 * Decreases must snap: removing a finished delete reflows layout by one
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

const STACK_SHIFT_VAR = "--thumbnail-stack-shift";
const STACK_SHIFTING_CLASS = "thumbnail-stack-shifting";
const STACK_SHIFT_INSTANT_CLASS = "thumbnail-stack-shift-instant";

function isDustDeleteCard(card: HTMLElement): boolean {
  return card.classList.contains("thumbnail-exit-delete")
    && card.classList.contains("thumbnail-exit-dust");
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

function writeStackShiftPx(card: HTMLElement, shiftPx: number, animate: boolean): void {
  if (shiftPx <= 0) {
    card.classList.remove(STACK_SHIFTING_CLASS);
    card.classList.remove(STACK_SHIFT_INSTANT_CLASS);
    card.style.removeProperty(STACK_SHIFT_VAR);
    return;
  }

  if (!animate) {
    card.classList.add(STACK_SHIFT_INSTANT_CLASS);
    card.classList.add(STACK_SHIFTING_CLASS);
    card.style.setProperty(STACK_SHIFT_VAR, `${shiftPx}px`);
    // Force the browser to commit the snapped value before re-enabling easing
    // so a later increase still transitions from the correct origin.
    void card.offsetWidth;
    card.classList.remove(STACK_SHIFT_INSTANT_CLASS);
    return;
  }

  card.classList.remove(STACK_SHIFT_INSTANT_CLASS);
  card.classList.add(STACK_SHIFTING_CLASS);
  card.style.setProperty(STACK_SHIFT_VAR, `${shiftPx}px`);
}

/**
 * Drive multi-slot stack collapse for dust deletes.
 *
 * Pure CSS can only animate a single fixed 184px step. When several cards
 * below a survivor are deleting, the shift must stack (N × slot). When a
 * finished delete is removed, the layout reflow and transform must update
 * together without an intermediate ease-back.
 */
export function createThumbnailStackShiftController(stack: HTMLElement): () => void {
  const deleteStartedAt = new WeakMap<HTMLElement, number>();
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
      if (!isDustDeleteCard(card)) continue;
      if (deleteStartedAt.has(card)) continue;
      deleteStartedAt.set(card, now);
      // Wake once this slot becomes motion-ready (plus a frame of slack).
      schedule(THUMBNAIL_STACK_MOTION_DELAY_MS + 16);
    }

    const motionStates: ThumbnailStackCardMotionState[] = cards.map((card) => {
      const deleteDust = isDustDeleteCard(card);
      const startedAt = deleteStartedAt.get(card);
      const motionReady = deleteDust
        && startedAt !== undefined
        && now - startedAt >= THUMBNAIL_STACK_MOTION_DELAY_MS;
      return {
        exiting: isExitingCard(card),
        deleteDust,
        motionReady,
      };
    });

    const shifts = computeThumbnailStackShifts(motionStates);
    for (let index = 0; index < cards.length; index += 1) {
      const card = cards[index]!;
      const nextPx = shifts[index] ?? 0;
      const previousPx = readStackShiftPx(card);
      if (previousPx === nextPx) {
        if (nextPx > 0) card.classList.add(STACK_SHIFTING_CLASS);
        continue;
      }
      writeStackShiftPx(
        card,
        nextPx,
        shouldAnimateThumbnailStackShift(previousPx, nextPx),
      );
    }
  };

  // Coalesce to one apply per turn, but stay before paint so a finished delete's
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

  // Initial sync in case deletes already started before the controller bound.
  queueApply();

  return () => {
    observer.disconnect();
    clearTimers();
    for (const card of stack.querySelectorAll<HTMLElement>(":scope > .thumbnail-card")) {
      card.classList.remove(STACK_SHIFTING_CLASS);
      card.classList.remove(STACK_SHIFT_INSTANT_CLASS);
      card.style.removeProperty(STACK_SHIFT_VAR);
    }
  };
}
