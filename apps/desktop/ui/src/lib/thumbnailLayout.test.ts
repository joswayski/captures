import {
  computeThumbnailStackShifts,
  countMotionReadyDeleteSlotsBelow,
  shouldAnimateThumbnailStackShift,
  shouldScrollThumbnailStackToEnd,
  thumbnailStackShiftPx,
  THUMBNAIL_CARD_SLOT_PX,
  type ThumbnailStackCardMotionState,
} from "./thumbnailLayout";

function card(
  partial: Partial<ThumbnailStackCardMotionState>,
): ThumbnailStackCardMotionState {
  return {
    exiting: false,
    deleteDust: false,
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

  it("counts only motion-ready dust deletes below a live card", () => {
    const cards = [
      card({}), // 1
      card({ exiting: true, deleteDust: true, motionReady: true }), // 2
      card({ exiting: true, deleteDust: true, motionReady: false }), // 3 not ready
      card({}), // 4
    ];
    expect(countMotionReadyDeleteSlotsBelow(cards, 0)).toBe(1);
    expect(countMotionReadyDeleteSlotsBelow(cards, 3)).toBe(0);
  });

  it("stacks shift distance across multiple ready deletes", () => {
    // 1 live, 2+3 deleting and ready, 4 live — card 1 must move two slots.
    const cards = [
      card({}),
      card({ exiting: true, deleteDust: true, motionReady: true }),
      card({ exiting: true, deleteDust: true, motionReady: true }),
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

  it("slides every live card above a bottom pair of deletes by two slots", () => {
    // Delete 3 and 4: 1 and 2 both need a 2-slot settle, not a 1-slot hop.
    const cards = [
      card({}),
      card({}),
      card({ exiting: true, deleteDust: true, motionReady: true }),
      card({ exiting: true, deleteDust: true, motionReady: true }),
    ];
    expect(computeThumbnailStackShifts(cards)).toEqual([
      thumbnailStackShiftPx(2),
      thumbnailStackShiftPx(2),
      0,
      0,
    ]);
  });

  it("ignores not-yet-ready deletes so early motion only accounts for mature holes", () => {
    const cards = [
      card({}),
      card({ exiting: true, deleteDust: true, motionReady: true }),
      card({ exiting: true, deleteDust: true, motionReady: false }),
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
});
