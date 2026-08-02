import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  createThumbnailStackShiftController,
  computeThumbnailStackShifts,
  countMotionReadySlotsBelow,
  shouldAnimateThumbnailStackShift,
  shouldScrollThumbnailStackToEnd,
  thumbnailStackOverflow,
  thumbnailStackShiftPx,
  THUMBNAIL_CARD_SLOT_PX,
  THUMBNAIL_DISMISS_HOLD_MS,
  THUMBNAIL_DISMISS_STACK_MOTION_DELAY_MS,
  THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS,
  THUMBNAIL_STACK_MOTION_DURATION_MS,
  THUMBNAIL_STACK_SETTLE_MAX_WAIT_MS,
  waitForThumbnailStackSettle,
  type ThumbnailStackCardMotionState,
} from "./thumbnailLayout";

const thumbnailStyles = readFileSync(
  resolve(process.cwd(), "ui/src/styles.css"),
  "utf8",
);

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
  it("releases the arrival animation before cards exit or shift", () => {
    const arrival = thumbnailStyles.match(
      /\.thumbnail-card\.thumbnail-ready([^{}]*)\{([^{}]*animation:\s*thumbnail-arrive[^{}]*)\}/,
    );

    expect(arrival?.[1]).toContain(":not(.thumbnail-exiting)");
    expect(arrival?.[2]).not.toMatch(/\b(?:both|forwards)\b/);
  });

  it("scrolls to reveal newly added captures", () => {
    expect(shouldScrollThumbnailStackToEnd(1, 2)).toBe(true);
  });

  it("does not force a second scroll after a capture closes", () => {
    expect(shouldScrollThumbnailStackToEnd(2, 1)).toBe(false);
    expect(shouldScrollThumbnailStackToEnd(2, 2)).toBe(false);
  });

  it("reports hidden previews at each scroll edge", () => {
    expect(thumbnailStackOverflow(0, 1_000, 400)).toEqual({
      hasOlder: false,
      hasNewer: true,
    });
    expect(thumbnailStackOverflow(300, 1_000, 400)).toEqual({
      hasOlder: true,
      hasNewer: true,
    });
    expect(thumbnailStackOverflow(600, 1_000, 400)).toEqual({
      hasOlder: true,
      hasNewer: false,
    });
    expect(thumbnailStackOverflow(0, 400, 400)).toEqual({
      hasOlder: false,
      hasNewer: false,
    });
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

  it("does not rewrite a settled shift class from its own mutation observer", async () => {
    vi.useFakeTimers();
    const originalMutationObserver = globalThis.MutationObserver;
    const observer = { callback: null as MutationCallback | null };
    class ControlledMutationObserver {
      constructor(callback: MutationCallback) {
        observer.callback = callback;
      }

      observe() {}
      disconnect() {}
      takeRecords(): MutationRecord[] { return []; }
    }
    globalThis.MutationObserver = ControlledMutationObserver as unknown as typeof MutationObserver;

    const stack = document.createElement("main");
    const survivor = document.createElement("article");
    survivor.className = "thumbnail-card";
    const exiting = document.createElement("article");
    exiting.className = "thumbnail-card thumbnail-exiting thumbnail-exit-delete thumbnail-exit-dust";
    stack.append(survivor, exiting);
    const dispose = createThumbnailStackShiftController(stack);

    try {
      await Promise.resolve();
      vi.advanceTimersByTime(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      expect(survivor).toHaveClass("thumbnail-stack-shifting");

      const add = vi.spyOn(survivor.classList, "add");
      const callback = observer.callback;
      if (!callback) throw new Error("stack controller did not create a mutation observer");
      callback([], {} as MutationObserver);
      await Promise.resolve();

      expect(add).not.toHaveBeenCalled();
    } finally {
      dispose();
      globalThis.MutationObserver = originalMutationObserver;
      vi.useRealTimers();
    }
  });

  it("follows a survivor transition when another exit retargets it", async () => {
    const controlledTransition = () => {
      let playState: AnimationPlayState = "running";
      let resolveFinished: (animation: Animation) => void = () => undefined;
      let rejectFinished: (reason?: unknown) => void = () => undefined;
      const finished = new Promise<Animation>((resolve, reject) => {
        resolveFinished = resolve;
        rejectFinished = reject;
      });
      const animation = {
        get finished() {
          return finished;
        },
        get playState() {
          return playState;
        },
        transitionProperty: "transform",
      } as unknown as Animation;
      return {
        animation,
        cancel: () => {
          playState = "idle";
          rejectFinished(new DOMException("Retargeted", "AbortError"));
        },
        finish: () => {
          playState = "finished";
          resolveFinished(animation);
        },
      };
    };

    const stack = document.createElement("main");
    const survivor = document.createElement("article");
    survivor.className = "thumbnail-card thumbnail-stack-shifting";
    const exiting = document.createElement("article");
    exiting.className = "thumbnail-card thumbnail-exit-dismiss thumbnail-exiting";
    stack.append(survivor, exiting);
    document.body.append(stack);
    const first = controlledTransition();
    const second = controlledTransition();
    let active = first.animation;
    Object.defineProperty(survivor, "getAnimations", {
      configurable: true,
      value: () => [active],
    });

    try {
      let settled = false;
      const wait = waitForThumbnailStackSettle(exiting).then(() => {
        settled = true;
      });
      await Promise.resolve();

      active = second.animation;
      first.cancel();
      await Promise.resolve();
      await Promise.resolve();
      expect(settled).toBe(false);

      second.finish();
      await wait;
      expect(settled).toBe(true);
    } finally {
      stack.remove();
    }
  });

  it("bounds the settle wait when a hidden WebView pauses transitions", async () => {
    vi.useFakeTimers();
    const stack = document.createElement("main");
    const survivor = document.createElement("article");
    survivor.className = "thumbnail-card thumbnail-stack-shifting";
    const exiting = document.createElement("article");
    exiting.className = "thumbnail-card thumbnail-exit-delete thumbnail-exiting";
    stack.append(survivor, exiting);
    document.body.append(stack);
    const pausedTransition = {
      finished: new Promise<Animation>(() => undefined),
      pending: false,
      playState: "running",
      transitionProperty: "transform",
    } as unknown as Animation;
    Object.defineProperty(survivor, "getAnimations", {
      configurable: true,
      value: () => [pausedTransition],
    });

    try {
      let settled = false;
      const wait = waitForThumbnailStackSettle(exiting).then(() => {
        settled = true;
      });
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_SETTLE_MAX_WAIT_MS);
      await wait;
      expect(settled).toBe(true);
    } finally {
      stack.remove();
      vi.useRealTimers();
    }
  });
});
