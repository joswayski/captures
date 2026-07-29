import {
  computeThumbnailStackShifts,
  countMotionReadySlotsBelow,
  shouldAnimateThumbnailStackShift,
  shouldScrollThumbnailStackToEnd,
  thumbnailStackShiftPx,
  THUMBNAIL_CARD_SLOT_PX,
  THUMBNAIL_DISMISS_HOLD_MS,
  THUMBNAIL_DISMISS_STACK_MOTION_DELAY_MS,
  THUMBNAIL_STACK_MOTION_DURATION_MS,
  type ThumbnailStackCardMotionState,
} from "./thumbnailLayout";

function card(
  partial: Partial<ThumbnailStackCardMotionState>,
): ThumbnailStackCardMotionState {
  return {
    exiting: false,
    holdsLayoutSlot: false,
    motionReady: false,
    ...partial,
  };
}

describe("thumbnail stack layout", () => {
  it("scrolls to reveal newly added captures", () => {
    expect(shouldScrollThumbnailStackToEnd(1, 2)).toBe(true);
  });

  it("does not force a second scroll after a capture closes", () => {
    expect(shouldScrollThumbnailStackToEnd(2, 1)).toBe(false);
    expect(shouldScrollThumbnailStackToEnd(2, 2)).toBe(false);
  });

  it("counts only motion-ready held-layout exits below a live card", () => {
    const cards = [
      card({}), // 1
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }), // 2
      card({ exiting: true, holdsLayoutSlot: true, motionReady: false }), // 3 not ready
      card({}), // 4
    ];
    expect(countMotionReadySlotsBelow(cards, 0)).toBe(1);
    expect(countMotionReadySlotsBelow(cards, 3)).toBe(0);
  });

  it("stacks shift distance across multiple ready deletes", () => {
    // 1 live, 2+3 deleting and ready, 4 live — card 1 must move two slots.
    const cards = [
      card({}),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
      card({}),
    ];
    expect(computeThumbnailStackShifts(cards)).toEqual([
      thumbnailStackShiftPx(2),
      0,
      0,
      0,
    ]);
    expect(thumbnailStackShiftPx(2)).toBe(THUMBNAIL_CARD_SLOT_PX * 2);
  });

  it("stacks dismiss exits the same way as deletes", () => {
    // Close 2 and 3: survivors above need a two-slot settle, not flex reflow.
    const cards = [
      card({}),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
      card({}),
    ];
    expect(computeThumbnailStackShifts(cards)[0]).toBe(thumbnailStackShiftPx(2));
  });

  it("slides every live card above a bottom pair of exits by two slots", () => {
    // Delete/dismiss 3 and 4: 1 and 2 both need a 2-slot settle.
    const cards = [
      card({}),
      card({}),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
    ];
    expect(computeThumbnailStackShifts(cards)).toEqual([
      thumbnailStackShiftPx(2),
      thumbnailStackShiftPx(2),
      0,
      0,
    ]);
  });

  it("ignores not-yet-ready exits so early motion only accounts for mature holes", () => {
    const cards = [
      card({}),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: false }),
      card({}),
    ];
    expect(computeThumbnailStackShifts(cards)).toEqual([
      thumbnailStackShiftPx(1),
      0,
      0,
      0,
    ]);
  });

  it("animates only when the required shift increases", () => {
    expect(shouldAnimateThumbnailStackShift(0, THUMBNAIL_CARD_SLOT_PX)).toBe(true);
    expect(shouldAnimateThumbnailStackShift(THUMBNAIL_CARD_SLOT_PX, THUMBNAIL_CARD_SLOT_PX * 2))
      .toBe(true);
    // Slot removal reflows layout; transform must snap down to cancel the jump.
    expect(shouldAnimateThumbnailStackShift(THUMBNAIL_CARD_SLOT_PX * 2, THUMBNAIL_CARD_SLOT_PX))
      .toBe(false);
    expect(shouldAnimateThumbnailStackShift(THUMBNAIL_CARD_SLOT_PX, 0)).toBe(false);
    expect(shouldAnimateThumbnailStackShift(THUMBNAIL_CARD_SLOT_PX, THUMBNAIL_CARD_SLOT_PX))
      .toBe(false);
  });

  it("holds dismiss layout long enough for the shared settle ease", () => {
    expect(THUMBNAIL_DISMISS_HOLD_MS).toBe(
      THUMBNAIL_DISMISS_STACK_MOTION_DELAY_MS + THUMBNAIL_STACK_MOTION_DURATION_MS,
    );
    expect(THUMBNAIL_STACK_MOTION_DURATION_MS).toBe(580);
  });
});
