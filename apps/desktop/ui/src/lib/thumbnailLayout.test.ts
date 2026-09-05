import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  animateThumbnailStackScroll,
  createThumbnailStackShiftController,
  computeThumbnailStackShifts,
  countMotionReadySlotsBelow,
  thumbnailStackSuppressesSlotShift,
  thumbnailStackShiftSlots,
  easeOutCubic,
  resolveThumbnailStackShiftPx,
  shouldAnimateThumbnailStackShift,
  scheduleScrollThumbnailStackToNewest,
  shouldScrollThumbnailStackToEnd,
  shouldScrollThumbnailStackToNewestOnExpand,
  thumbnailStackContentHeight,
  thumbnailStackMotionClassNames,
  thumbnailStackNeedsScrollport,
  restoreThumbnailStackShiftClass,
  thumbnailStackOverflow,
  thumbnailStackNewestScrollTop,
  scrollThumbnailStackToNewest,
  THUMBNAIL_STACK_SCROLLPORT_CLASS,
  thumbnailStackShiftPx,
  thumbnailCollapsedPeekPx,
  thumbnailCollapsedFrameHeight,
  thumbnailStackGravityFromNormalizedY,
  thumbnailStackGravityFromHarness,
  thumbnailStackGravityFromWorkArea,
  thumbnailStackAnchorFromGravity,
  thumbnailStackHarnessPileTop,
  convertHarnessStackOffsetAnchor,
  convertThumbnailStackFrameAnchor,
  thumbnailStackVisualPileBottom,
  applyThumbnailStackGravity,
  harnessOffsetForPlacement,
  thumbnailStackAnchorFromPlacement,
  thumbnailStackGravityFromPlacement,
  THUMBNAIL_STACK_GRAVITY_VAR,
  THUMBNAIL_STACK_ANCHOR_TOP_GRAVITY,
  THUMBNAIL_STACK_ANCHOR_BOTTOM_GRAVITY,
  captureThumbnailCardTransforms,
  thumbnailStackFanCollapseMs,
  thumbnailStackPeekJitterPx,
  thumbnailStackPileDepth,
  thumbnailStackPoseDepth,
  thumbnailStackPoseStep,
  THUMBNAIL_CARD_HEIGHT_PX,
  THUMBNAIL_CARD_SLOT_PX,
  THUMBNAIL_DISMISS_HOLD_MS,
  THUMBNAIL_DISMISS_STACK_MOTION_DELAY_MS,
  THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS,
  THUMBNAIL_STACK_CONTROL_GUTTER_PX,
  THUMBNAIL_STACK_EXPAND_COLLAPSE_MS,
  THUMBNAIL_MINIMIZE_MORPH_MS,
  THUMBNAIL_MINIMIZE_SWAP_MS,
  THUMBNAIL_STACK_GAP_PX,
  THUMBNAIL_STACK_IDLE_PEEK_PX,
  THUMBNAIL_STACK_HOVER_PEEK_PX,
  THUMBNAIL_STACK_MOTION_DURATION_MS,
  THUMBNAIL_STACK_PADDING_PX,
  THUMBNAIL_STACK_PEEK_JITTER_DECAY,
  THUMBNAIL_STACK_PEEK_JITTER_PX,
  THUMBNAIL_STACK_RECEDING_STEP,
  THUMBNAIL_STACK_SCROLL_DURATION_MS,
  THUMBNAIL_STACK_SETTLE_MAX_WAIT_MS,
  waitForThumbnailStackSettle,
  type ThumbnailStackCardMotionState,
} from "./thumbnailLayout";

const thumbnailStyles = readFileSync(
  resolve(process.cwd(), "ui/src/styles/mini-preview.css"),
  "utf8",
);
const designTokens = readFileSync(
  resolve(process.cwd(), "../../shared/design.css"),
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
  it("does not ease compact-card transforms until collapse hover is armed", () => {
    const compactCard = thumbnailStyles.match(
      /\.thumbnail-stack-compact > \.thumbnail-card\s*\{([\s\S]*?)\n\}/,
    );
    const hoverReady = thumbnailStyles.match(
      /\.thumbnail-stack-minimized\.thumbnail-stack-hover-ready > \.thumbnail-card\s*\{([\s\S]*?)\n\}/,
    );
    const minimizingCard = thumbnailStyles.match(
      /\.thumbnail-stack-minimizing > \.thumbnail-card\s*\{\n {2}animation: none;([\s\S]*?)\n\}/,
    );
    const minimizeRun = thumbnailStyles.match(
      /^\.thumbnail-stack-minimizing\.thumbnail-stack-minimize-run > \.thumbnail-card\s*\{([\s\S]*?)\n\}/m,
    );
    const hoverFan = thumbnailStyles.match(
      /\.thumbnail-stack-minimized\.thumbnail-stack-hover-ready:not\(\.thumbnail-stack-hover-latched\):not\(\.thumbnail-stack-dragging\):not\(\.thumbnail-stack-pressing\):has\(\.thumbnail-collapsed-hit-target:hover:not\(\[data-thumbnail-hover-stale\]\)\)/,
    );

    const pressing = thumbnailStyles.match(
      /\.thumbnail-stack-minimized\.thumbnail-stack-pressing > \.thumbnail-card,\s*\n\.thumbnail-stack-minimized\.thumbnail-stack-dragging:not\(\.thumbnail-stack-drag-sway\) > \.thumbnail-card\s*\{([\s\S]*?)\n\}/,
    );

    expect(compactCard?.[1]).toMatch(/--thumbnail-stack-pile-depth/);
    expect(compactCard?.[1]).toMatch(/--thumbnail-stack-base-depth/);
    expect(compactCard?.[1]).toMatch(/--thumbnail-stack-pose/);
    expect(compactCard?.[1]).toMatch(/--thumbnail-stack-shift-slots/);
    expect(compactCard?.[1]).toMatch(/--thumbnail-stack-peek-jitter/);
    expect(compactCard?.[1]).toMatch(/transform:\s*var\(--thumbnail-stack-rest-transform\)/);
    expect(compactCard?.[1]).toMatch(/translate:\s*none\s*!important/);
    expect(compactCard?.[1]).not.toMatch(/--thumbnail-stack-skew-/);
    expect(compactCard?.[1]).not.toMatch(/--thumbnail-stack-fan-tilt/);
    expect(compactCard?.[1]).not.toMatch(/--thumbnail-stack-fan-shift/);
    expect(thumbnailStyles).toMatch(/--thumbnail-stack-gravity/);
    expect(thumbnailStyles).toMatch(/--thumbnail-stack-expand-sign:\s*-1/);
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-anchor-top\.thumbnail-stack-compact > \.thumbnail-card\s*\{[^}]*top:\s*52px/,
    );
    expect(thumbnailStyles).toMatch(
      /var\(--thumbnail-stack-pile-depth, 0\) \* -9px\s*\n\s*\* var\(--thumbnail-stack-gravity, 1\)/,
    );
    expect(compactCard?.[1]).toMatch(/--thumbnail-stack-hover-transform/);
    expect(compactCard?.[1]).toMatch(/rotateZ\(0deg\)/);
    expect(compactCard?.[1]).toMatch(/--thumbnail-stack-expanded-transform/);
    expect(compactCard?.[1]).not.toMatch(/transform\s+var\(--stack-fan-dur\)/);
    expect(hoverReady?.[1]).toMatch(
      /transform\s+var\(--stack-fan-dur\) calc\(var\(--thumbnail-stack-pile-depth, 0\) \* var\(--stack-fan-stagger\)\)/,
    );
    expect(pressing?.[1]).toMatch(/transition:/);
    expect(pressing?.[1]).toMatch(
      /transform\s+var\(--stack-fan-dur\) calc\(var\(--thumbnail-stack-pile-depth, 0\) \* var\(--stack-fan-stagger\)\)/,
    );
    expect(minimizingCard?.[1]).toMatch(/var\(--thumbnail-stack-expanded-transform\)/);
    expect(minimizeRun?.[1]).toMatch(/transform:\s*var\(--thumbnail-stack-rest-transform\)/);
    expect(minimizeRun?.[1]).toMatch(/transform 0\.52s/);
    expect(minimizeRun?.[1]).toMatch(/top 0\.52s/);
    expect(thumbnailStyles).toMatch(/thumbnail-card-expand 0\.52s/);
    expect(thumbnailStyles).toMatch(/thumbnail-card-minimize-dim 0\.52s/);
    expect(thumbnailStyles).toMatch(/thumbnail-card-expand-dim 0\.52s/);
    expect(hoverFan).not.toBeNull();
    expect(thumbnailStyles).toMatch(/--stack-fan-stagger:\s*16ms/);
    expect(thumbnailStyles).not.toMatch(/--stack-pile-max-depth:\s*3/);
    expect(thumbnailStyles).toMatch(
      /transform:\s*var\(--thumbnail-stack-expand-from, var\(--thumbnail-stack-rest-transform\)\)/,
    );
    expect(thumbnailStyles).not.toMatch(/@keyframes thumbnail-card-expand-from-hover/);
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-scrollport\s*\{[^}]*height:\s*100%/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-compact\.thumbnail-stack-expanding\s*\{[^}]*overflow:\s*hidden/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack\s*\{[^}]*scroll-behavior:\s*auto/,
    );
  });

  it("captures live card transforms so expand can start from a partial pose", () => {
    const stack = document.createElement("main");
    const card = document.createElement("article");
    card.className = "thumbnail-card";
    card.setAttribute("data-thumbnail-id", "capture-1");
    stack.append(card);
    const computed = { transform: "matrix(0.97, 0.12, -0.12, 0.97, 10, -24)" };
    const spy = vi.spyOn(window, "getComputedStyle").mockReturnValue(
      computed as CSSStyleDeclaration,
    );

    expect(captureThumbnailCardTransforms(stack).get("capture-1")).toBe(computed.transform);
    expect(captureThumbnailCardTransforms(stack).has("missing")).toBe(false);

    spy.mockImplementation(() => ({ transform: "none" }) as CSSStyleDeclaration);
    expect(captureThumbnailCardTransforms(stack).size).toBe(0);
    spy.mockRestore();
  });

  it("keeps collapsed cards square so drag sway and gravity own the lean", () => {
    expect(thumbnailStyles).not.toMatch(/--thumbnail-stack-skew-/);
    expect(thumbnailStyles).not.toMatch(/--thumbnail-stack-fan-tilt/);
    expect(thumbnailStyles).not.toMatch(/--thumbnail-stack-fan-shift/);
    expect(thumbnailStyles).not.toMatch(/--thumbnail-collapsed-skew-pad/);
    expect(thumbnailStackFanCollapseMs(1)).toBe(200);
    expect(thumbnailStackFanCollapseMs(4)).toBeCloseTo(
      200 + thumbnailStackPoseDepth(3) * 16,
    );
    expect(thumbnailStackFanCollapseMs(12)).toBeCloseTo(
      200 + thumbnailStackPoseDepth(11) * 16,
    );
  });

  it("tucks structured peek at mid-screen and inverts it at the top", () => {
    expect(thumbnailStackGravityFromNormalizedY(1)).toBe(1);
    expect(thumbnailStackGravityFromNormalizedY(0.5)).toBe(0);
    expect(thumbnailStackGravityFromNormalizedY(0)).toBe(-1);
    expect(thumbnailCollapsedFrameHeight(1)).toBe(240);
    expect(thumbnailCollapsedFrameHeight(4)).toBe(
      THUMBNAIL_STACK_PADDING_PX
      + THUMBNAIL_STACK_CONTROL_GUTTER_PX
      + THUMBNAIL_CARD_HEIGHT_PX
      + Math.max(0, thumbnailCollapsedPeekPx(4, true) - THUMBNAIL_STACK_PADDING_PX),
    );

    const viewportHeight = 800;
    const contentHeight = 240;
    expect(thumbnailStackGravityFromHarness({
      offsetY: 0,
      anchor: "bottom",
      viewportHeight,
      contentHeight,
    })).toBeCloseTo(1);
    expect(thumbnailStackGravityFromHarness({
      offsetY: contentHeight - viewportHeight,
      anchor: "bottom",
      viewportHeight,
      contentHeight,
    })).toBeCloseTo(-1);
    expect(thumbnailStackHarnessPileTop({
      offsetY: contentHeight - viewportHeight,
      anchor: "bottom",
      viewportHeight,
      contentHeight,
    })).toBeCloseTo(0);

    const bottomOffset = { x: 12, y: contentHeight - viewportHeight };
    const topOffset = convertHarnessStackOffsetAnchor(
      bottomOffset,
      "bottom",
      "top",
      viewportHeight,
      contentHeight,
    );
    expect(topOffset).toEqual({ x: 12, y: 0 });
    expect(convertHarnessStackOffsetAnchor(
      topOffset,
      "top",
      "bottom",
      viewportHeight,
      contentHeight,
    )).toEqual(bottomOffset);

    expect(convertThumbnailStackFrameAnchor(
      { x: 40, y: -552 },
      "bottom",
      "top",
      792,
      240,
    )).toEqual({ x: 40, y: 0 });
    expect(convertThumbnailStackFrameAnchor(
      { x: 40, y: 0 },
      "top",
      "bottom",
      792,
      240,
    )).toEqual({ x: 40, y: -552 });

    expect(thumbnailStackAnchorFromPlacement("top_right")).toBe("top");
    expect(thumbnailStackGravityFromPlacement("top_left")).toBe(-1);
    expect(harnessOffsetForPlacement("bottom_right", { width: 1_280, height: 720 })).toEqual({
      x: 940,
      y: 0,
      anchor: "bottom",
    });
    expect(harnessOffsetForPlacement("top_left", { width: 1_280, height: 720 }).anchor).toBe("top");
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-anchor-top\s*\{[^}]*padding:\s*52px 28px 28px/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-anchor-top\.thumbnail-stack-compact > \.thumbnail-card\s*\{[^}]*top:\s*52px/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar-anchor-top\s*\{[^}]*top:\s*16px/,
    );

    expect(thumbnailStackAnchorFromGravity(-0.5, "bottom")).toBe("top");
    expect(thumbnailStackAnchorFromGravity(-0.1, "bottom")).toBe("bottom");
    expect(thumbnailStackAnchorFromGravity(0.5, "top")).toBe("bottom");
    expect(thumbnailStackAnchorFromGravity(0.1, "top")).toBe("top");
    expect(THUMBNAIL_STACK_ANCHOR_TOP_GRAVITY).toBeLessThan(0);
    expect(THUMBNAIL_STACK_ANCHOR_BOTTOM_GRAVITY).toBeGreaterThan(0);

    expect(thumbnailStackGravityFromWorkArea({
      pileBottom: 1_040,
      workTop: 0,
      workHeight: 1_040,
      contentHeight: 240,
    })).toBeCloseTo(1);
    expect(thumbnailStackGravityFromWorkArea({
      pileBottom: 240,
      workTop: 0,
      workHeight: 1_040,
      contentHeight: 240,
    })).toBeCloseTo(-1);

    const tallFrame = 792;
    const topOfScreen = convertThumbnailStackFrameAnchor(
      { x: 12, y: contentHeight - tallFrame },
      "bottom",
      "top",
      tallFrame,
      contentHeight,
    );
    expect(topOfScreen).toEqual({ x: 12, y: 0 });
    expect(convertThumbnailStackFrameAnchor(
      topOfScreen,
      "top",
      "bottom",
      tallFrame,
      contentHeight,
    )).toEqual({ x: 12, y: contentHeight - tallFrame });
    expect(thumbnailStackVisualPileBottom({
      y: contentHeight - tallFrame,
      frameHeight: tallFrame,
      contentHeight,
      anchor: "bottom",
    })).toBe(contentHeight);
    expect(thumbnailStackVisualPileBottom({
      y: 0,
      frameHeight: tallFrame,
      contentHeight,
      anchor: "top",
    })).toBe(contentHeight);
    expect(thumbnailStackGravityFromWorkArea({
      pileBottom: thumbnailStackVisualPileBottom({
        y: 0,
        frameHeight: tallFrame,
        contentHeight,
        anchor: "top",
      }),
      workTop: 0,
      workHeight: 1_040,
      contentHeight,
    })).toBeCloseTo(-1);

    const stack = document.createElement("main");
    applyThumbnailStackGravity(stack, -0.42);
    expect(stack.style.getPropertyValue(THUMBNAIL_STACK_GRAVITY_VAR)).toBe("-0.42");
    expect(thumbnailStyles).toMatch(
      /var\(--thumbnail-stack-pile-depth, 0\) \* -9px\s*\n\s*\* var\(--thumbnail-stack-gravity, 1\)/,
    );
    expect(thumbnailStyles).not.toMatch(/--thumbnail-stack-skew-y/);
  });

  it("recedes extra collapsed cards instead of hiding them", () => {
    expect(thumbnailStackPileDepth(0)).toBe(0);
    expect(thumbnailStackPileDepth(3)).toBeCloseTo(thumbnailStackPoseDepth(3));
    expect(thumbnailStackPileDepth(8)).toBeGreaterThan(thumbnailStackPileDepth(3));
    expect(thumbnailStackPileDepth(8)).toBeLessThan(8);
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-expanding > \.thumbnail-card\s*\{[^}]*opacity:\s*1/,
    );
    expect(thumbnailStyles).toMatch(/--thumbnail-stack-pose-k:\s*24/);
    expect(thumbnailStyles).toMatch(/--thumbnail-stack-recede:\s*0.55/);
    expect(thumbnailStyles).not.toMatch(/--thumbnail-stack-extra/);
  });

  it("arches the collapsed pile so the top trails the hands while dragging", () => {
    const dragging = thumbnailStyles.match(
      /\.thumbnail-stack-minimized\.thumbnail-stack-dragging\.thumbnail-stack-drag-sway > \.thumbnail-card\s*\{([\s\S]*?)\n\}/,
    );

    expect(dragging?.[1]).not.toMatch(/transform\s+0\.22s/);
    expect(dragging?.[1]).not.toMatch(/40ms/);
    expect(dragging?.[1]).toMatch(/--thumbnail-drag-sway-x/);
    expect(dragging?.[1]).toMatch(/--thumbnail-stack-pile-depth/);
    expect(dragging?.[1]).toMatch(/\* 0\.7/);
    expect(dragging?.[1]).toMatch(/\* 0\.2/);
    expect(dragging?.[1]).not.toMatch(/\(var\(--thumbnail-stack-pile-depth, 0\) \+ 1\)/);
    expect(dragging?.[1]).not.toMatch(/--thumbnail-stack-skew-/);
    expect(dragging?.[1]).toMatch(/rotateZ\(/);
    expect(dragging?.[1]).not.toMatch(/-0\.28deg/);
    expect(dragging?.[1]).not.toMatch(/0\.18deg/);
    expect(dragging?.[1]).toMatch(/0\.12deg/);
  });

  it("releases the arrival animation before cards exit or shift", () => {
    const arrival = thumbnailStyles.match(
      /\.thumbnail-card\.thumbnail-ready([^{}]*)\{([^{}]*animation:\s*thumbnail-arrive[^{}]*)\}/,
    );

    expect(arrival?.[1]).toContain(":not(.thumbnail-exiting)");
    expect(arrival?.[2]).not.toMatch(/\b(?:both|forwards)\b/);
  });

  it("keeps the blurred source above delete dust during the handoff", () => {
    const sourceLayer = thumbnailStyles.match(
      /\.thumbnail-exit-delete\.thumbnail-exit-dust \.thumbnail-media\s*\{[^}]*z-index:\s*(\d+)/,
    );
    const dustLayer = thumbnailStyles.match(
      /\.thumbnail-exit-delete\.thumbnail-exit-dust \.thumbnail-dust-layer\s*\{[^}]*z-index:\s*(\d+)/,
    );
    const sourceFade = thumbnailStyles.match(
      /@keyframes thumbnail-delete-img-fade\s*\{([\s\S]*?)\n\}/,
    );

    expect(Number(sourceLayer?.[1])).toBeGreaterThan(Number(dustLayer?.[1]));
    expect(sourceFade?.[1]).toMatch(/filter:\s*blur\(2px\) brightness\(0\.5\)/);
    expect(sourceFade?.[1]).toMatch(/0%,\s*20%/);
  });

  it("keeps hovered delete chrome on the first dissolve frame", () => {
    const deleteTooltip = thumbnailStyles.match(
      /\.thumbnail-exit-delete \.icon-button\.delete::after\s*\{([^}]*)\}/,
    );
    const deleteHover = thumbnailStyles.match(
      /\.thumbnail-exit-delete \.icon-button\.delete\s*\{([^}]*)\}/,
    );
    const disabledSave = thumbnailStyles.match(
      /\.thumbnail-exit-delete \.thumbnail-main-actions button:disabled,\s*\.thumbnail-exit-delete \.icon-button:disabled\s*\{([^}]*)\}/,
    );
    const bottomBar = thumbnailStyles.match(
      /\.thumbnail-exit-delete \.thumbnail-bottom-bar\s*\{([^}]*)\}/,
    );
    const dustChip = thumbnailStyles.match(
      /\.thumbnail-dust\s*\{([^}]*)\}/,
    );

    expect(thumbnailStyles).toMatch(
      /\.thumbnail-card\.thumbnail-exiting:not\(\.thumbnail-exit-delete\) \.icon-button::after/,
    );
    expect(deleteTooltip?.[1]).toMatch(/opacity:\s*1/);
    expect(deleteHover?.[1]).toMatch(/background:\s*var\(--theme-signal\)/);
    expect(disabledSave?.[1]).toMatch(/opacity:\s*1/);
    expect(bottomBar?.[1]).toMatch(/z-index:\s*8/);
    expect(dustChip?.[1]).toMatch(/filter:\s*blur\(2px\) brightness\(0\.5\)/);
  });

  it("fades the mini-preview frame with the delete dissolve", () => {
    const deleteRule = thumbnailStyles.match(
      /\.thumbnail-card\.thumbnail-exit-delete\s*\{([\s\S]*?)\n\}/,
    );
    const frameFade = thumbnailStyles.match(
      /@keyframes thumbnail-delete-frame-fade\s*\{([\s\S]*?)\n\}/,
    );
    const outlineFade = thumbnailStyles.match(
      /\.thumbnail-card\.thumbnail-exit-delete::after\s*\{([^}]*)\}/,
    );

    expect(deleteRule?.[1]).toMatch(/thumbnail-delete-frame-fade\s+0\.5s/);
    expect(deleteRule?.[1]).toMatch(/0 0 0 1px rgba\(255, 255, 255, 0\)/);
    expect(deleteRule?.[1]).not.toMatch(/^\s*box-shadow:\s*none/m);
    expect(frameFade?.[1]).toMatch(/from\s*\{/);
    expect(frameFade?.[1]).toMatch(/0 0 0 1px rgba\(255, 255, 255, 0\.08\)/);
    expect(frameFade?.[1]).toMatch(/0 0 0 1px rgba\(255, 255, 255, 0\)/);
    expect(outlineFade?.[1]).toMatch(/filter:\s*opacity\(0\)/);
  });

  it("uses a pointer cursor on the collapsed stack so expand is obvious", () => {
    const hitTarget = thumbnailStyles.match(
      /\.thumbnail-collapsed-hit-target\s*\{([\s\S]*?)\n\}/,
    );

    expect(hitTarget?.[1]).toMatch(/cursor:\s*pointer/);
    expect(hitTarget?.[1]).toMatch(/--thumbnail-collapsed-peek/);
    expect(hitTarget?.[1]).toMatch(/left:\s*28px/);
    expect(hitTarget?.[1]).toMatch(/right:\s*28px/);
    expect(hitTarget?.[1]).not.toMatch(/--thumbnail-collapsed-skew-pad/);
    expect(hitTarget?.[1]).not.toMatch(/height:\s*248px/);
    expect(thumbnailStyles).toMatch(/thumbnail-stack-sparkle/);
    expect(thumbnailStyles).not.toMatch(/thumbnail-stack-expand-path/);
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-minimized(?::not\(\.thumbnail-stack-dragging\))? > \.thumbnail-card \*/,
    );
    expect(thumbnailStyles).toMatch(
      /html\.thumbnail-native-tracking \.thumbnail-stack-minimized \.thumbnail-card img/,
    );
    expect(thumbnailStyles).toMatch(
      /html:has\(\.thumbnail-card:hover\)\s*\{[\s\S]*?cursor:\s*grab/,
    );
    expect(thumbnailStyles).toMatch(
      /html:has\(\s*:is\(\s*\.thumbnail-stack-control/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-card:hover :is\(button, \.icon-button, \.thumbnail-editor-control\):not\(:disabled\)/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-control:hover:not\(:disabled\),\s*\n\.thumbnail-stack-control:hover:not\(:disabled\) \*/,
    );
  });

  it("fades the Show less control in linearly and delays hiding it on last delete", () => {
    const enter = thumbnailStyles.split("@keyframes thumbnail-stack-toolbar-in")[1]
      ?.split("@keyframes")[0];
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar-entering\s*\{[^}]*animation:\s*thumbnail-stack-toolbar-in 0\.52s linear/,
    );
    expect(enter).toMatch(/35%/);
    expect(enter).not.toMatch(/60%/);
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar-exiting\s*\{[^}]*animation:\s*thumbnail-stack-toolbar-exit 1\.05s/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-control\s*\{[\s\S]*?cursor:\s*pointer/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-control \*,\s*\n\.thumbnail-stack-minimize,\s*\n\.thumbnail-stack-minimize \*\s*\{[\s\S]*?cursor:\s*pointer/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-collapsed-hit-target:not\(:disabled\) \*\s*\{[\s\S]*?cursor:\s*pointer/,
    );
    expect(thumbnailStyles).toMatch(/--thumbnail-minimize-morph:\s*240ms/);
    expect(thumbnailStyles).toMatch(/--thumbnail-minimize-swap:\s*180ms/);
    expect(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS).toBe(520);
    expect(THUMBNAIL_MINIMIZE_MORPH_MS).toBe(240);
    expect(THUMBNAIL_MINIMIZE_SWAP_MS).toBe(180);
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar\s*\{[\s\S]*?position:\s*fixed/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar:not\(\.thumbnail-stack-toolbar-leaving\):not\(\.thumbnail-stack-toolbar-exiting\):not\(\.thumbnail-stack-toolbar-entering\) \.thumbnail-stack-minimize:hover/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar:not\(\.thumbnail-stack-toolbar-leaving\):not\(\.thumbnail-stack-toolbar-exiting\):not\(\.thumbnail-stack-toolbar-entering\) \.thumbnail-stack-minimize:focus-visible \{[\s\S]*?cursor:\s*pointer[\s\S]*?transform:\s*translateY\(-1px\)/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar:not\(\.thumbnail-stack-toolbar-leaving\):not\(\.thumbnail-stack-toolbar-exiting\):not\(\.thumbnail-stack-toolbar-entering\) \.thumbnail-stack-minimize:active/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar-leaving \.thumbnail-stack-minimize\s*\{[^}]*width:\s*92px/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar-leaving \.thumbnail-stack-minimize-label\s*\{[^}]*opacity:\s*1/,
    );
    const leave = thumbnailStyles.split("@keyframes thumbnail-stack-toolbar-out")[1]
      ?.split("@keyframes")[0];
    expect(designTokens).toMatch(/--blur-dissolve:\s*8px;/);
    expect(leave).toMatch(/filter:\s*blur\(var\(--blur-dissolve\)\)/);
    expect(leave).not.toMatch(/translateY/);
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar-exiting \.thumbnail-stack-minimize,[\s\S]*?\{[^}]*width:\s*28px/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar-exiting \.thumbnail-stack-minimize,[\s\S]*?\{[^}]*transition:\s*none/,
    );
  });

  it("reserves the Show less gutter above expanded cards at the top of the screen", () => {
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack\s*\{[\s\S]*?padding:\s*28px 28px 52px/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-anchor-top:not\(\.thumbnail-stack-compact\)\s*\{[\s\S]*?padding:\s*52px 28px 28px/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar-anchor-top\s*\{[\s\S]*?top:\s*16px/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-top-actions\s*\{[\s\S]*?top:\s*8px/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-anchor-top\.thumbnail-stack-compact > \.thumbnail-card\s*\{[\s\S]*?top:\s*28px/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-anchor-top\.thumbnail-stack-expanding > \.thumbnail-card,\s*\n\.thumbnail-stack-anchor-top\.thumbnail-stack-minimizing > \.thumbnail-card\s*\{[\s\S]*?top:\s*52px/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-anchor-top\.thumbnail-stack-minimizing\.thumbnail-stack-minimize-run > \.thumbnail-card\s*\{[\s\S]*?top:\s*28px/,
    );
  });

  it("centers mini-preview tooltips on their icons and flips them away from the stack edge", () => {
    const iconTip = thumbnailStyles.match(/\.icon-button::after\s*\{([^}]*)\}/);
    const editorTip = thumbnailStyles.match(/\.thumbnail-editor-control-tip\s*\{([^}]*)\}/);
    const stackTip = thumbnailStyles.match(
      /\.thumbnail-stack-control\[data-tooltip\]::after\s*\{([^}]*)\}/,
    );
    const deleteFreeze = thumbnailStyles.match(
      /\.thumbnail-exit-delete \.icon-button\.delete::after\s*\{([^}]*)\}/,
    );

    expect(iconTip?.[1]).toMatch(/left:\s*50%/);
    expect(iconTip?.[1]).toMatch(/translate:\s*-50% 0/);
    expect(iconTip?.[1]).toMatch(/justify-self:\s*start/);
    expect(editorTip?.[1]).toMatch(/left:\s*50%/);
    expect(editorTip?.[1]).toMatch(/translate:\s*-50% 0/);
    expect(stackTip?.[1]).toMatch(/left:\s*50%/);
    expect(stackTip?.[1]).toMatch(/translate:\s*-50% 0/);
    expect(stackTip?.[1]).toMatch(/bottom:\s*calc\(100% \+ 6px\)/);
    expect(deleteFreeze?.[1]).toMatch(/transform:\s*translateY\(0\)/);

    expect(thumbnailStyles).not.toMatch(/\.thumbnail-top-left \.icon-button::after/);
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-top-actions \.icon-button::after\s*\{[\s\S]*?bottom:\s*calc\(100% \+ 6px\)/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-anchor-top \.thumbnail-top-actions \.icon-button::after\s*\{[\s\S]*?top:\s*calc\(100% \+ 6px\)/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-anchor-top \.thumbnail-editor-control-tip\s*\{[\s\S]*?top:\s*calc\(100% \+ 6px\)/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-toolbar-anchor-top \.thumbnail-stack-control\[data-tooltip\]::after\s*\{[\s\S]*?top:\s*calc\(100% \+ 6px\)/,
    );
  });

  it("sizes the collapsed expand target from receding extra cards", () => {
    expect(thumbnailCollapsedPeekPx(1)).toBe(0);
    expect(thumbnailCollapsedPeekPx(2)).toBeCloseTo(
      thumbnailStackPoseDepth(1) * THUMBNAIL_STACK_IDLE_PEEK_PX,
    );
    expect(thumbnailCollapsedPeekPx(4)).toBeCloseTo(
      thumbnailStackPoseDepth(3) * THUMBNAIL_STACK_IDLE_PEEK_PX,
    );
    expect(thumbnailCollapsedPeekPx(8)).toBeGreaterThan(thumbnailCollapsedPeekPx(4));
    expect(thumbnailCollapsedPeekPx(8)).toBeCloseTo(
      thumbnailStackPoseDepth(7) * THUMBNAIL_STACK_IDLE_PEEK_PX,
    );
    expect(thumbnailCollapsedPeekPx(8) - thumbnailCollapsedPeekPx(4)).toBeCloseTo(
      (thumbnailStackPoseDepth(7) - thumbnailStackPoseDepth(3)) * THUMBNAIL_STACK_IDLE_PEEK_PX,
    );
    expect(thumbnailCollapsedPeekPx(24)).toBeGreaterThan(thumbnailCollapsedPeekPx(8));
    expect(thumbnailCollapsedPeekPx(2, true)).toBeCloseTo(
      thumbnailStackPoseDepth(1) * THUMBNAIL_STACK_HOVER_PEEK_PX,
    );
    expect(thumbnailCollapsedPeekPx(4, true)).toBeCloseTo(
      thumbnailStackPoseDepth(3) * THUMBNAIL_STACK_HOVER_PEEK_PX,
    );
    expect(thumbnailCollapsedPeekPx(8, true)).toBeCloseTo(
      thumbnailStackPoseDepth(7) * THUMBNAIL_STACK_HOVER_PEEK_PX,
    );
    expect(thumbnailCollapsedPeekPx(1, true)).toBe(0);
  });

  it("eases collapsed peeks so the fade-out does not open a gap", () => {
    expect(thumbnailStackPoseDepth(0)).toBe(0);
    expect(Math.abs(thumbnailStackPoseStep(4) - thumbnailStackPoseStep(3))).toBeLessThan(0.08);
    expect(thumbnailStackPoseStep(4)).toBeGreaterThan(THUMBNAIL_STACK_RECEDING_STEP);
    expect(thumbnailStackPoseStep(8)).toBeLessThan(thumbnailStackPoseStep(3));
    expect(thumbnailStackPoseStep(20)).toBeGreaterThan(THUMBNAIL_STACK_RECEDING_STEP);
    expect(thumbnailStackPoseDepth(20)).toBeLessThan(20);
  });

  it("nudges collapsed peeks a little less toward the back of the pile", () => {
    expect(thumbnailStackPeekJitterPx(0)).toBe(0);
    expect(Math.abs(thumbnailStackPeekJitterPx(1))).toBeGreaterThan(0);
    expect(Math.abs(thumbnailStackPeekJitterPx(1))).toBeLessThanOrEqual(
      THUMBNAIL_STACK_PEEK_JITTER_PX,
    );
    const envelope = (depth: number) => (
      THUMBNAIL_STACK_PEEK_JITTER_PX * THUMBNAIL_STACK_PEEK_JITTER_DECAY ** (depth - 1)
    );
    expect(envelope(6)).toBeLessThan(envelope(2));
    expect(Math.abs(thumbnailStackPeekJitterPx(6))).toBeLessThanOrEqual(envelope(6) + 1e-12);
    expect(thumbnailStyles).toMatch(
      /var\(--thumbnail-stack-pile-depth, 0\) \* -9px\s*\n\s*\* var\(--thumbnail-stack-gravity, 1\)\s*\+\s*var\(--thumbnail-stack-peek-jitter, 0px\)/,
    );
  });

  it("scrolls to reveal newly added captures", () => {
    expect(shouldScrollThumbnailStackToEnd(1, 2)).toBe(true);
  });

  it("does not force a second scroll after a capture closes", () => {
    expect(shouldScrollThumbnailStackToEnd(2, 1)).toBe(false);
    expect(shouldScrollThumbnailStackToEnd(2, 2)).toBe(false);
  });

  it("scrolls to the newest capture when the pile expands", () => {
    expect(shouldScrollThumbnailStackToNewestOnExpand("collapsed", "expanded")).toBe(true);
    expect(shouldScrollThumbnailStackToNewestOnExpand("expanding", "expanded")).toBe(true);
    expect(shouldScrollThumbnailStackToNewestOnExpand("expanded", "expanded")).toBe(false);
    expect(shouldScrollThumbnailStackToNewestOnExpand(undefined, "expanded")).toBe(false);
    expect(shouldScrollThumbnailStackToNewestOnExpand("expanded", "collapsing")).toBe(false);
  });

  it("pins newest-scroll to layout height instead of paint overflow", () => {
    expect(thumbnailStackNewestScrollTop(1, 400)).toBe(0);
    expect(thumbnailStackNewestScrollTop(8, 400)).toBe(
      thumbnailStackContentHeight(8) - 400,
    );
    expect(thumbnailStackNewestScrollTop(8, 400, true)).toBe(0);
  });

  it("fills the viewport when the expanded list is taller than the window", () => {
    expect(thumbnailStackNeedsScrollport(1, 400)).toBe(false);
    expect(thumbnailStackNeedsScrollport(8, 400)).toBe(true);
    expect(thumbnailStackNeedsScrollport(8, 10_000)).toBe(false);
  });

  it("pins newest-scroll in the same pass when the stack is still content-sized", () => {
    const stack = document.createElement("main");
    stack.innerHTML = "<article class=\"thumbnail-card\"></article>".repeat(8);
    Object.defineProperty(stack, "clientHeight", {
      configurable: true,
      value: thumbnailStackContentHeight(8),
    });
    Object.defineProperty(stack, "scrollTop", {
      configurable: true,
      writable: true,
      value: 0,
    });

    scrollThumbnailStackToNewest(stack, { viewportHeight: 400 });

    expect(stack.classList.contains(THUMBNAIL_STACK_SCROLLPORT_CLASS)).toBe(true);
    expect(stack.scrollTop).toBe(thumbnailStackNewestScrollTop(8, 400));

    scrollThumbnailStackToNewest(stack, { viewportHeight: 400, fromTop: true });
    expect(stack.classList.contains(THUMBNAIL_STACK_SCROLLPORT_CLASS)).toBe(true);
    expect(stack.scrollTop).toBe(0);
  });

  it("retries newest-scroll after the window grows", () => {
    const stack = document.createElement("main");
    stack.innerHTML = "<article class=\"thumbnail-card\"></article>".repeat(8);
    Object.defineProperty(stack, "clientHeight", {
      configurable: true,
      writable: true,
      value: 200,
    });
    Object.defineProperty(stack, "scrollTop", {
      configurable: true,
      writable: true,
      value: 0,
    });

    const frames: FrameRequestCallback[] = [];
    const cancel = scheduleScrollThumbnailStackToNewest(stack, {
      retryMs: 50,
      viewportHeight: 400,
      frame: (callback) => {
        frames.push(callback);
        return frames.length;
      },
      cancelFrame: (id) => {
        frames[id - 1] = () => undefined;
      },
    });

    expect(stack.scrollTop).toBe(thumbnailStackNewestScrollTop(8, 200));
    Object.defineProperty(stack, "clientHeight", {
      configurable: true,
      value: 400,
    });
    frames[0]?.(0);
    expect(stack.scrollTop).toBe(thumbnailStackNewestScrollTop(8, 400));
    cancel();
  });

  it("dims stacked cards with an overlay instead of a parent filter", () => {
    expect(thumbnailStyles).not.toMatch(
      /\.thumbnail-stack-compact > \.thumbnail-card\s*\{[^}]*filter:\s*brightness/,
    );
    const overlay = thumbnailStyles.match(
      /\.thumbnail-stack-compact > \.thumbnail-card::before\s*\{([^}]*)\}/,
    );
    expect(overlay?.[1]).toMatch(
      /opacity:\s*calc\(min\(0\.72, var\(--thumbnail-stack-pile-depth/,
    );
    expect(overlay?.[1]).toMatch(/background:\s*var\(--glass-strong-solid\)/);
    expect(overlay?.[1]).not.toMatch(/background:\s*#000/);
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack-minimized > \.thumbnail-card \.thumbnail-media\s*\{[^}]*blur\(calc\(var\(--thumbnail-stack-pose/,
    );
    expect(thumbnailStyles).not.toMatch(/--thumbnail-stack-hidden/);
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack\[data-thumbnail-suppress-card-hover="true"\]/,
    );
  });

  it("keeps keyboard-focused card actions visible while hover is locked", () => {
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack\[data-thumbnail-suppress-card-hover="true"\] \.thumbnail-card:hover:not\(:focus-within\)/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-stack\[data-thumbnail-suppress-card-hover="true"\] \.thumbnail-card\[data-thumbnail-native-active="true"\]:not\(:focus-within\)/,
    );
    expect(thumbnailStyles).not.toMatch(
      /\.thumbnail-stack\[data-thumbnail-suppress-card-hover="true"\] \.thumbnail-card:focus-within/,
    );
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
      THUMBNAIL_STACK_PADDING_PX
        + THUMBNAIL_STACK_CONTROL_GUTTER_PX
        + THUMBNAIL_CARD_HEIGHT_PX,
    );
    expect(thumbnailStackContentHeight(4)).toBe(
      THUMBNAIL_STACK_PADDING_PX
        + THUMBNAIL_STACK_CONTROL_GUTTER_PX
        + 4 * THUMBNAIL_CARD_HEIGHT_PX
        + 3 * THUMBNAIL_STACK_GAP_PX,
    );
  });

  it("ignores inflated scrollable overflow when layout content fits", () => {
    // Four cards fill a 792px stack. Dust chips and settle transforms can make
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

  it("keeps live cards behind a neighbor that froze mid-settle", () => {
    // Delete 3, then delete 2 before 2 finishes sliding into 3. 1 must stay
    // behind 2 instead of completing the slide into a still-solid preview.
    const cards = [
      card({}),
      card({
        exiting: true,
        holdsLayoutSlot: true,
        motionReady: false,
        currentShiftPx: thumbnailStackShiftPx(1) * 0.85,
      }),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
      card({}),
    ];
    expect(computeThumbnailStackShifts(cards)).toEqual([
      thumbnailStackShiftPx(1) * 0.85,
      thumbnailStackShiftPx(1) * 0.85,
      0,
      0,
    ]);
  });

  it("does not slide a live card into a deleting neighbor that already settled into a lower hole", () => {
    // 2 already occupies 3's slot. 1 may sit in 2's vacated layout slot, but
    // must not take a second slot until 2 is dissolving in place.
    const cards = [
      card({ currentShiftPx: thumbnailStackShiftPx(1) }),
      card({
        exiting: true,
        holdsLayoutSlot: true,
        motionReady: false,
        currentShiftPx: thumbnailStackShiftPx(1),
      }),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
      card({}),
    ];
    expect(computeThumbnailStackShifts(cards)).toEqual([
      thumbnailStackShiftPx(1),
      thumbnailStackShiftPx(1),
      0,
      0,
    ]);
  });

  it("waits to consume a shifted deleting neighbor until that neighbor is a clear hole", () => {
    const cards = [
      card({ currentShiftPx: thumbnailStackShiftPx(1) }),
      card({
        exiting: true,
        holdsLayoutSlot: true,
        motionReady: true,
        currentShiftPx: thumbnailStackShiftPx(1),
      }),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
      card({}),
    ];
    expect(computeThumbnailStackShifts(cards)).toEqual([
      thumbnailStackShiftPx(1),
      thumbnailStackShiftPx(1),
      0,
      0,
    ]);
  });

  it("slides into a deleting neighbor once that neighbor is dissolving in its layout slot", () => {
    const cards = [
      card({}),
      card({ exiting: true, holdsLayoutSlot: true, motionReady: true }),
      card({}),
    ];
    expect(computeThumbnailStackShifts(cards)).toEqual([
      thumbnailStackShiftPx(1),
      0,
      0,
    ]);
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

  it("holds a live card behind a mid-settle delete instead of sliding into it", async () => {
    vi.useFakeTimers();
    const stack = document.createElement("main");
    const first = document.createElement("article");
    first.className = "thumbnail-card";
    const second = document.createElement("article");
    second.className = "thumbnail-card";
    const third = document.createElement("article");
    third.className = "thumbnail-card thumbnail-exiting thumbnail-exit-delete thumbnail-exit-dust";
    const fourth = document.createElement("article");
    fourth.className = "thumbnail-card";
    stack.append(first, second, third, fourth);
    document.body.append(stack);
    const dispose = createThumbnailStackShiftController(stack);

    try {
      await Promise.resolve();
      vi.advanceTimersByTime(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      expect(first).toHaveClass("thumbnail-stack-shifting");
      expect(second).toHaveClass("thumbnail-stack-shifting");
      expect(first.style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );
      expect(second.style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );

      second.classList.add("thumbnail-exiting", "thumbnail-exit-delete", "thumbnail-exit-dust");
      await Promise.resolve();
      await Promise.resolve();

      expect(first.style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );
      expect(second.style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );

      vi.advanceTimersByTime(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      expect(first.style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );
      expect(second.style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );
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

  it("treats a compact pile as suppressing expanded slot shifts", () => {
    const stack = document.createElement("main");
    stack.className = "thumbnail-stack";
    expect(thumbnailStackSuppressesSlotShift(stack)).toBe(false);
    stack.classList.add("thumbnail-stack-compact");
    expect(thumbnailStackSuppressesSlotShift(stack)).toBe(true);
    expect(thumbnailStackShiftSlots(THUMBNAIL_CARD_SLOT_PX)).toBe(1);
    expect(thumbnailStackShiftSlots(0)).toBe(0);
  });

  it("clears slot shifts while the stack is compact and restores them after expand", async () => {
    vi.useFakeTimers();
    const stack = document.createElement("main");
    stack.className = "thumbnail-stack";
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
      expect(survivor.style.translate).toBe(`0 ${THUMBNAIL_CARD_SLOT_PX}px`);

      stack.classList.add("thumbnail-stack-compact");
      await Promise.resolve();
      await Promise.resolve();
      expect(survivor).not.toHaveClass("thumbnail-stack-shifting");
      expect(survivor.style.getPropertyValue("--thumbnail-stack-shift")).toBe("");
      expect(survivor.style.translate).toBe("");
      expect(survivor.style.getPropertyValue("--thumbnail-stack-shift-slots")).toBe("1");

      vi.advanceTimersByTime(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS);
      await Promise.resolve();
      expect(survivor.style.translate).toBe("");
      expect(survivor.style.getPropertyValue("--thumbnail-stack-shift-slots")).toBe("1");

      stack.classList.remove("thumbnail-stack-compact");
      await Promise.resolve();
      await Promise.resolve();
      expect(survivor).toHaveClass("thumbnail-stack-shifting");
      expect(survivor.style.translate).toBe(`0 ${THUMBNAIL_CARD_SLOT_PX}px`);
      expect(survivor.style.getPropertyValue("--thumbnail-stack-shift-slots")).toBe("");
    } finally {
      dispose();
      vi.useRealTimers();
    }
  });

  it("does not start a slot shift while a delete plays on a compact pile", async () => {
    vi.useFakeTimers();
    const stack = document.createElement("main");
    stack.className = "thumbnail-stack thumbnail-stack-compact";
    const survivor = document.createElement("article");
    survivor.className = "thumbnail-card";
    const exiting = document.createElement("article");
    exiting.className = "thumbnail-card thumbnail-exiting thumbnail-exit-delete thumbnail-exit-dust";
    stack.append(survivor, exiting);
    const dispose = createThumbnailStackShiftController(stack);

    try {
      await Promise.resolve();
      vi.advanceTimersByTime(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      expect(survivor).not.toHaveClass("thumbnail-stack-shifting");
      expect(survivor.style.translate).toBe("");
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
