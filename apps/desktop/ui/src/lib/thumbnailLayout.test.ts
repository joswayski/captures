import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  animateThumbnailStackScroll,
  createThumbnailStackShiftController,
  computeThumbnailStackShifts,
  countMotionReadySlotsBelow,
  easeOutCubic,
  resolveThumbnailStackShiftPx,
  shouldAnimateThumbnailStackShift,
  shouldScrollThumbnailStackToEnd,
  thumbnailStackContentHeight,
  thumbnailStackMotionClassNames,
  restoreThumbnailStackShiftClass,
  thumbnailStackOverflow,
  thumbnailStackShiftPx,
  THUMBNAIL_CARD_HEIGHT_PX,
  THUMBNAIL_CARD_SLOT_PX,
  THUMBNAIL_DISMISS_HOLD_MS,
  THUMBNAIL_DISMISS_STACK_MOTION_DELAY_MS,
  THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS,
  THUMBNAIL_STACK_GAP_PX,
  THUMBNAIL_STACK_MOTION_DURATION_MS,
  THUMBNAIL_STACK_PADDING_PX,
  THUMBNAIL_STACK_SCROLL_DURATION_MS,
  THUMBNAIL_STACK_SETTLE_MAX_WAIT_MS,
  waitForThumbnailStackSettle,
  type ThumbnailStackCardMotionState,
} from "./thumbnailLayout";

const thumbnailStyles = readFileSync(
  resolve(process.cwd(), "ui/src/styles/mini-preview.css"),
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

  it("computes stack content height from card layout, not paint overflow", () => {
    expect(thumbnailStackContentHeight(0)).toBe(0);
    expect(thumbnailStackContentHeight(1)).toBe(
      THUMBNAIL_STACK_PADDING_PX * 2 + THUMBNAIL_CARD_HEIGHT_PX,
    );
    expect(thumbnailStackContentHeight(4)).toBe(
      THUMBNAIL_STACK_PADDING_PX * 2
        + 4 * THUMBNAIL_CARD_HEIGHT_PX
        + 3 * THUMBNAIL_STACK_GAP_PX,
    );
  });

  it("ignores inflated scrollable overflow when layout content fits", () => {
    // Four cards fill a 768px stack. Dust chips and settle transforms can make
    // WebKit report a taller scrollHeight; cues must use layout height instead
    // so the bottom drawer does not flash while survivors settle.
    const layoutHeight = thumbnailStackContentHeight(4);
    const clientHeight = layoutHeight;
    expect(thumbnailStackOverflow(0, layoutHeight, clientHeight)).toEqual({
      hasOlder: false,
      hasNewer: false,
    });
    // Same scrollTop against paint-inflated height would incorrectly show hasNewer.
    expect(thumbnailStackOverflow(0, layoutHeight + 240, clientHeight)).toEqual({
      hasOlder: false,
      hasNewer: true,
    });
  });

  it("eases overflow-cue scrolls out of the target", () => {
    expect(easeOutCubic(0)).toBe(0);
    expect(easeOutCubic(1)).toBe(1);
    // Ease-out progresses faster than linear early, then decelerates.
    expect(easeOutCubic(0.5)).toBeGreaterThan(0.5);
  });

  it("animates stack scroll with ease-out and can cancel mid-flight", () => {
    const stack = document.createElement("main");
    Object.defineProperty(stack, "scrollTop", {
      configurable: true,
      writable: true,
      value: 600,
    });

    const frames: FrameRequestCallback[] = [];
    let clock = 0;
    const cancel = animateThumbnailStackScroll(stack, 416, {
      durationMs: THUMBNAIL_STACK_SCROLL_DURATION_MS,
      now: () => clock,
      frame: (callback) => {
        frames.push(callback);
        return frames.length;
      },
      cancelFrame: () => {
        frames.length = 0;
      },
    });

    expect(frames).toHaveLength(1);
    clock = THUMBNAIL_STACK_SCROLL_DURATION_MS * 0.5;
    frames.at(-1)?.(clock);
    expect(stack.scrollTop).toBeLessThan(600);
    expect(stack.scrollTop).toBeGreaterThan(416);

    const mid = stack.scrollTop;
    cancel();
    clock = THUMBNAIL_STACK_SCROLL_DURATION_MS;
    frames.at(-1)?.(clock);
    // Cancel freezes at the last interpolated value.
    expect(stack.scrollTop).toBe(mid);
  });

  it("jumps immediately when reduced motion is preferred", () => {
    const stack = document.createElement("main");
    Object.defineProperty(stack, "scrollTop", {
      configurable: true,
      writable: true,
      value: 600,
    });
    animateThumbnailStackScroll(stack, 416, { reducedMotion: true });
    expect(stack.scrollTop).toBe(416);
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

  it("clamps exiting cards to their current shift so they cannot jump up or chase new holes", () => {
    expect(resolveThumbnailStackShiftPx(thumbnailStackShiftPx(1), thumbnailStackShiftPx(1), true))
      .toBe(thumbnailStackShiftPx(1));
    expect(resolveThumbnailStackShiftPx(thumbnailStackShiftPx(1), 0, true)).toBe(0);
    expect(resolveThumbnailStackShiftPx(0, thumbnailStackShiftPx(1), true)).toBe(0);
    expect(resolveThumbnailStackShiftPx(thumbnailStackShiftPx(1), 0, false))
      .toBe(thumbnailStackShiftPx(1));
  });

  it("keeps an exiting card's existing shift until holes below it close", () => {
    const cards = [
      card({ exiting: true, currentShiftPx: thumbnailStackShiftPx(1) }),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
    ];
    expect(computeThumbnailStackShifts(cards)).toEqual([
      thumbnailStackShiftPx(1),
      0,
    ]);
  });

  it("does not pull an already-exiting card into a hole that opens later", () => {
    const cards = [
      card({ exiting: true, currentShiftPx: 0 }),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
    ];
    expect(computeThumbnailStackShifts(cards)).toEqual([0, 0]);
  });

  it("snaps an exiting card's shift down when a hole below is removed", () => {
    const cards = [
      card({ exiting: true, currentShiftPx: thumbnailStackShiftPx(1) }),
      card({}),
    ];
    expect(computeThumbnailStackShifts(cards)).toEqual([0, 0]);
  });

  it("shifts stacked cards with the translate property so dismiss transform can compose", () => {
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-card\.thumbnail-stack-shifting\s*\{[^}]*translate:\s*0 var\(--thumbnail-stack-shift/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-card\.thumbnail-stack-shifting\.thumbnail-exiting\s*\{[^}]*filter:\s*none/,
    );
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

  it("keeps a settled survivor's shift when that card itself starts exiting", async () => {
    vi.useFakeTimers();
    const stack = document.createElement("main");
    const survivor = document.createElement("article");
    survivor.className = "thumbnail-card";
    const exiting = document.createElement("article");
    exiting.className = "thumbnail-card thumbnail-exiting thumbnail-exit-delete thumbnail-exit-dust";
    stack.append(survivor, exiting);
    document.body.append(stack);
    const dispose = createThumbnailStackShiftController(stack);

    try {
      await Promise.resolve();
      vi.advanceTimersByTime(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      expect(survivor).toHaveClass("thumbnail-stack-shifting");
      expect(survivor.style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );
      expect(survivor.style.translate).toBe(`0 ${THUMBNAIL_CARD_SLOT_PX}px`);

      // React className rewrites drop controller tokens unless the card copies
      // them back. Simulate the wipe, then the exit classes landing.
      survivor.className = "thumbnail-card thumbnail-exit-delete thumbnail-exit-dust thumbnail-exiting";
      await Promise.resolve();
      await Promise.resolve();

      expect(survivor).toHaveClass("thumbnail-stack-shifting");
      expect(survivor).toHaveClass("thumbnail-exiting");
      expect(survivor.style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );
      expect(survivor.style.translate).toBe(`0 ${THUMBNAIL_CARD_SLOT_PX}px`);
    } finally {
      dispose();
      stack.remove();
      vi.useRealTimers();
    }
  });

  it("does not slide an already-exiting card when a hole below becomes ready", async () => {
    vi.useFakeTimers();
    const stack = document.createElement("main");
    const upper = document.createElement("article");
    upper.className = "thumbnail-card thumbnail-exiting thumbnail-exit-delete thumbnail-exit-dust";
    const lower = document.createElement("article");
    lower.className = "thumbnail-card thumbnail-exiting thumbnail-exit-delete thumbnail-exit-dust";
    stack.append(upper, lower);
    const dispose = createThumbnailStackShiftController(stack);

    try {
      await Promise.resolve();
      vi.advanceTimersByTime(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      expect(upper).not.toHaveClass("thumbnail-stack-shifting");
      expect(upper.style.getPropertyValue("--thumbnail-stack-shift")).toBe("");
    } finally {
      dispose();
      vi.useRealTimers();
    }
  });

  it("snaps an exiting survivor's shift off when the hole below is removed", async () => {
    vi.useFakeTimers();
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

      survivor.classList.add("thumbnail-exiting", "thumbnail-exit-delete", "thumbnail-exit-dust");
      await Promise.resolve();
      await Promise.resolve();
      expect(survivor.style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );

      exiting.remove();
      await Promise.resolve();
      await Promise.resolve();
      expect(survivor).not.toHaveClass("thumbnail-stack-shifting");
      expect(survivor.style.getPropertyValue("--thumbnail-stack-shift")).toBe("");
      expect(survivor.style.translate).toBe("");
    } finally {
      dispose();
      vi.useRealTimers();
    }
  });

  it("copies live stack-motion classes so React can preserve them", () => {
    const card = document.createElement("article");
    expect(thumbnailStackMotionClassNames(card)).toEqual([]);
    card.classList.add("thumbnail-stack-shifting");
    expect(thumbnailStackMotionClassNames(card)).toEqual(["thumbnail-stack-shifting"]);
    card.classList.add("thumbnail-stack-shift-instant");
    expect(thumbnailStackMotionClassNames(card)).toEqual([
      "thumbnail-stack-shifting",
      "thumbnail-stack-shift-instant",
    ]);
    expect(thumbnailStackMotionClassNames(null)).toEqual([]);
  });

  it("restores the shifting class from a leftover stack offset", () => {
    const card = document.createElement("article");
    card.style.setProperty("--thumbnail-stack-shift", `${THUMBNAIL_CARD_SLOT_PX}px`);
    restoreThumbnailStackShiftClass(card);
    expect(card).toHaveClass("thumbnail-stack-shifting");
    card.style.removeProperty("--thumbnail-stack-shift");
    card.classList.remove("thumbnail-stack-shifting");
    restoreThumbnailStackShiftClass(card);
    expect(card).not.toHaveClass("thumbnail-stack-shifting");
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
