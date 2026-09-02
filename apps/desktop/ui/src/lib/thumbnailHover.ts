import type { ThumbnailPointerPosition } from "../types";

/**
 * Reassert interactive preview cursors on every successful poll. macOS restores
 * the frontmost app's arrow while Captures is inactive; a 100ms throttle left a
 * visible default↔hand flash on each frame between reasserts.
 */
export const THUMBNAIL_CURSOR_REASSERT_INTERVAL_MS = 0;

/**
 * Extra reassert delays (ms) after a click or focus handoff. Immediate (0) covers
 * the next task; later ticks cover WebKit's post-click arrow install and the
 * key-window steal when Edit opens the screenshot editor. Native AppKit also
 * reasserts across the following main-queue turns so the arrow cannot flash
 * between these JS ticks.
 */
export const THUMBNAIL_CURSOR_HANDOFF_REASSERT_DELAYS_MS = [0, 8, 16, 48, 96] as const;

/** DOM marker mirroring the native cursor kind while pointer polling is active. */
export const THUMBNAIL_CURSOR_KIND_ATTRIBUTE = "data-thumbnail-cursor";

/**
 * Cap native pointer IPC so a hung invoke after sleep/resume cannot leave the
 * poll loop permanently locked (`polling === true` forever).
 */
export const THUMBNAIL_POINTER_POLL_TIMEOUT_MS = 400;

/**
 * After this many consecutive null/failed pointer samples, re-enable hit testing
 * so a pre-sleep `ignore_cursor_events(true)` cannot leave the stack frozen.
 * At the 40ms poll interval this is roughly half a second.
 */
export const THUMBNAIL_NULL_POLL_RECOVER_COUNT = 12;

const THUMBNAIL_NATIVE_ACTIVE_ATTRIBUTE = "data-thumbnail-native-active";
const THUMBNAIL_NATIVE_ACTIVE_SELECTOR = `[${THUMBNAIL_NATIVE_ACTIVE_ATTRIBUTE}="true"]`;
/**
 * Marker for the button under the native pointer. Stored as a data attribute
 * (not a React-managed class) so IconButton re-renders cannot wipe hover for a
 * frame and flash the AppKit arrow / hover chrome.
 */
export const THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE = "data-native-pointer-hover";
const THUMBNAIL_NATIVE_POINTER_HOVER_SELECTOR =
  `[${THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE}="true"]`;
const THUMBNAIL_STACK_HOVER_LATCHED_SELECTOR = ".thumbnail-stack-hover-latched";
const THUMBNAIL_STACK_CONTROL_SELECTOR = [
  ".thumbnail-overflow-cue",
  ".thumbnail-stack-control",
  ".thumbnail-collapsed-hit-target",
].join(", ");
/**
 * Marker on `.thumbnail-stack` while card hover chrome must stay idle after a
 * collapse/expand. Clicking the pile leaves the pointer over a card; applying
 * hover immediately plays blur + action fade on top of the expand motion.
 */
export const THUMBNAIL_SUPPRESS_CARD_HOVER_ATTRIBUTE = "data-thumbnail-suppress-card-hover";
const THUMBNAIL_SUPPRESS_CARD_HOVER_SELECTOR =
  `.thumbnail-stack[${THUMBNAIL_SUPPRESS_CARD_HOVER_ATTRIBUTE}="true"]`;
/** Ignore sub-pixel native samples so a stationary pointer cannot unlock hover. */
export const THUMBNAIL_CARD_HOVER_LOCK_SLOP_PX = 4;

/**
 * Keeps a freshly opened editor control in its passive “In editor” state until
 * the pointer actually leaves it. Without this latch, the stationary click
 * immediately counts as hover and the new status morphs straight into the
 * redundant “Show in editor” action label.
 */
export const THUMBNAIL_EDITOR_JUST_OPENED_ATTRIBUTE = "data-editor-just-opened";
const THUMBNAIL_EDITOR_JUST_OPENED_SELECTOR =
  `[${THUMBNAIL_EDITOR_JUST_OPENED_ATTRIBUTE}="true"]`;

/** Cursor kind for the always-on-top capture previews. */
export type ThumbnailCursorKind = "default" | "pointer" | "grab";

/**
 * Race an async poll against a timeout so sleep/resume cannot stall the loop.
 * Resolves with `null` on timeout (same as a transient unavailable sample).
 */
export function withThumbnailPointerTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number = THUMBNAIL_POINTER_POLL_TIMEOUT_MS,
): Promise<T | null> {
  return new Promise((resolve) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      resolve(null);
    }, Math.max(0, timeoutMs));
    promise.then(
      (value) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve(value);
      },
      () => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve(null);
      },
    );
  });
}

/** True when a run of empty pointer samples should force interaction recovery. */
export function shouldRecoverThumbnailAfterNullPolls(
  consecutiveNullOrFailed: number,
  threshold: number = THUMBNAIL_NULL_POLL_RECOVER_COUNT,
): boolean {
  return consecutiveNullOrFailed >= threshold;
}

export function thumbnailCursorSyncAction(
  current: ThumbnailCursorKind,
  next: ThumbnailCursorKind,
  elapsedMs: number,
  options: { force?: boolean } = {},
): "transition" | "reassert" | null {
  if (current !== next) return "transition";
  // macOS restores the frontmost app's arrow while Captures is inactive, and
  // also on mousedown/mouseup when a preview control is clicked. Keep
  // reasserting any interactive cursor (pointer on buttons, grab on the drag
  // source image) on every poll. Callers also pass `force` on pointer/focus
  // events so the hand is restored immediately around native handoffs.
  if (
    next !== "default"
    && (options.force || elapsedMs >= THUMBNAIL_CURSOR_REASSERT_INTERVAL_MS)
  ) {
    return "reassert";
  }
  return null;
}

export function thumbnailCssCursor(kind: ThumbnailCursorKind): string {
  if (kind === "pointer") return "pointer";
  if (kind === "grab") return "grab";
  return "default";
}

/**
 * Mirror the hit-tested cursor kind on the document so WebKit cursor rectangles
 * cannot alternate between element-level `pointer` / `grab` / default rules
 * while AppKit owns the real cursor.
 */
export function applyThumbnailCssCursor(
  kind: ThumbnailCursorKind,
  root: HTMLElement = document.documentElement,
) {
  // Default is a click-through hole in the always-on-top panel. Painting
  // `cursor: default` on the whole document lets WebKit cursor rectangles steal
  // hover cursors from whatever is now receiving those clicks.
  if (kind === "default") {
    clearThumbnailCssCursor(root);
    return;
  }
  const cssCursor = thumbnailCssCursor(kind);
  if (root.style.cursor !== cssCursor) {
    root.style.cursor = cssCursor;
  }
  if (root.getAttribute(THUMBNAIL_CURSOR_KIND_ATTRIBUTE) !== kind) {
    root.setAttribute(THUMBNAIL_CURSOR_KIND_ATTRIBUTE, kind);
  }
}

export function clearThumbnailCssCursor(
  root: HTMLElement = document.documentElement,
) {
  root.style.cursor = "";
  root.removeAttribute(THUMBNAIL_CURSOR_KIND_ATTRIBUTE);
}

/**
 * True when any preview card still needs mouse input.
 * Exiting cards keep a layout slot for the dissolve / dismiss animation, but
 * they must not keep the always-on-top window hit-testable — on Windows and
 * Linux that otherwise blocks the desktop for the whole ~3s delete.
 * Overflow cues only matter while a live card remains; an exiting-only stack
 * should pass every click through, including those controls.
 * Transitioning cards are decorative and pass clicks through. A minimized
 * stack is live only while its dedicated expand target remains enabled,
 * except while the pile is being dragged across the desktop.
 */
/** True while cards are still in the collapsed pile pose or its motion. */
export function thumbnailStackHoldsCollapsedPose(root: Document = document): boolean {
  return Boolean(root.querySelector(
    ".thumbnail-stack-compact, .thumbnail-stack-minimizing, .thumbnail-stack-expanding, .thumbnail-stack-minimized",
  ));
}

/**
 * True when preview cards must not show Copy/Save/Edit hover chrome. Compact
 * and animating stacks are decorative; after expand the suppress marker stays
 * until the pointer actually moves.
 */
export function thumbnailStackSuppressesCardHover(root: Document = document): boolean {
  return thumbnailStackHoldsCollapsedPose(root)
    || Boolean(root.querySelector(THUMBNAIL_SUPPRESS_CARD_HOVER_SELECTOR));
}

export function setThumbnailCardHoverSuppressed(
  suppressed: boolean,
  root: ParentNode = document,
) {
  const stack = root.querySelector(".thumbnail-stack");
  if (!stack) return;
  if (suppressed) {
    stack.setAttribute(THUMBNAIL_SUPPRESS_CARD_HOVER_ATTRIBUTE, "true");
  } else {
    stack.removeAttribute(THUMBNAIL_SUPPRESS_CARD_HOVER_ATTRIBUTE);
  }
}

/** True when a locked hover may resume — pointer left, or moved past slop. */
export function thumbnailCardHoverLockReleased(
  origin: { x: number; y: number } | null,
  position: ThumbnailPointerPosition,
  slopPx: number = THUMBNAIL_CARD_HOVER_LOCK_SLOP_PX,
): boolean {
  if (!position.inside) return true;
  if (!origin) return false;
  return Math.hypot(position.x - origin.x, position.y - origin.y) >= slopPx;
}

/**
 * Collapse, expand, and reduced-motion jumps all leave the pointer over a card
 * that was not hovered as a live preview. Keep hover locked through those
 * transitions. The initial expanded mount must not lock.
 */
export function shouldLockThumbnailCardHoverOnStackMotion(
  stackMotion: string | undefined,
  previousStackMotion: string | undefined,
): boolean {
  if (!stackMotion) return false;
  if (stackMotion !== "expanded") return true;
  return Boolean(previousStackMotion && previousStackMotion !== "expanded");
}

export function thumbnailStackHasLiveHitTarget(root: Document = document): boolean {
  if (root.querySelector(".thumbnail-stack-dragging")) return true;
  if (root.querySelector(
    ".thumbnail-stack-minimizing, .thumbnail-stack-expanding",
  )) {
    return false;
  }
  if (root.querySelector(".thumbnail-stack-minimized")) {
    return Boolean(root.querySelector(
      ".thumbnail-collapsed-hit-target:not(:disabled), .thumbnail-stack-control:not(:disabled)",
    ));
  }
  const cards = root.querySelectorAll(".thumbnail-card");
  for (const card of cards) {
    if (!card.classList.contains("thumbnail-exiting")) return true;
  }
  return false;
}

/**
 * Keep the native window interactive only over a live preview card, stack
 * overflow control, toolbar control, or minimized-stack target. After a dismiss it may stay tall
 * (shrinking blanks WKWebView), and a deleting card keeps its layout slot while
 * its particles finish. Empty space and exiting slots must pass clicks through
 * without disabling the remaining cards.
 *
 * When every card is exiting (or the stack is empty), ignore the cursor even
 * without a pointer sample. Platforms that cannot poll the cursor would
 * otherwise leave the native window blocking clicks until the animation ends.
 */
export function shouldIgnoreThumbnailCursorEvents(
  position: ThumbnailPointerPosition,
  root: Document = document,
): boolean {
  if (root.querySelector(".thumbnail-stack-dragging")) return false;
  if (!thumbnailStackHasLiveHitTarget(root)) return true;
  if (!position.inside) return false;
  const target = root.elementFromPoint(position.x, position.y);
  if (thumbnailStackControlAtPoint(position.x, position.y, target, root)) return false;
  if (!target) return true;
  const card = target.closest(".thumbnail-card");
  return !card || card.classList.contains("thumbnail-exiting");
}

export function clearThumbnailNativeHover(root: ParentNode = document) {
  root.querySelectorAll(
    `${THUMBNAIL_NATIVE_ACTIVE_SELECTOR}, ${THUMBNAIL_NATIVE_POINTER_HOVER_SELECTOR}`,
  )
    .forEach((element) => {
      element.removeAttribute(THUMBNAIL_NATIVE_ACTIVE_ATTRIBUTE);
      element.removeAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE);
    });
}

/**
 * Native pointer tracking is intentionally stored outside React's `className`.
 * Viewer activation rerenders the card and would otherwise overwrite an
 * imperatively-added class for one frame before the next pointer poll.
 */
export function setThumbnailNativeActiveCard(
  card: Element,
  root: ParentNode = document,
) {
  root.querySelectorAll(THUMBNAIL_NATIVE_ACTIVE_SELECTOR)
    .forEach((element) => {
      if (element !== card) {
        element.removeAttribute(THUMBNAIL_NATIVE_ACTIVE_ATTRIBUTE);
      }
    });
  card.setAttribute(THUMBNAIL_NATIVE_ACTIVE_ATTRIBUTE, "true");
}

export function markThumbnailEditorControlOpened(control: HTMLElement) {
  control.setAttribute(THUMBNAIL_EDITOR_JUST_OPENED_ATTRIBUTE, "true");
}

/**
 * End the just-opened latch so a later hover can show “Show in editor”.
 *
 * On leave (`fromLeave`), also drop residual hover/focus chrome in the same
 * tick. Otherwise pointerleave clears the latch while
 * `data-native-pointer-hover` (or `:focus-visible` from the click) still
 * matches for a frame or two, and the action label flashes before the next
 * poll / editor focus steal settles back to “In editor”.
 */
export function rearmThumbnailEditorControlHover(
  control: HTMLElement,
  options: { fromLeave?: boolean } = {},
) {
  control.removeAttribute(THUMBNAIL_EDITOR_JUST_OPENED_ATTRIBUTE);
  if (!options.fromLeave) return;
  control.removeAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE);
  if (document.activeElement === control) {
    control.blur();
  }
}

function containsPoint(element: Element, x: number, y: number): boolean {
  const bounds = element.getBoundingClientRect();
  return x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom;
}

/**
 * Finds stack chrome by its own geometry instead of relying only on
 * `elementFromPoint()`. The hide button overlaps the preview card by a few
 * pixels, and an inactive WKWebView can intermittently report that grab-source
 * card while the pointer is still inside the button. Treating the control's
 * stable bounds as authoritative prevents native pointer/grab cursor churn.
 */
function minimizedStackExpandControlAtPoint(
  x: number,
  y: number,
  directTarget: Element | null,
  root: Document,
): HTMLElement | null {
  const stack = root.querySelector<HTMLElement>(".thumbnail-stack-minimized");
  if (!stack) return null;
  const hitTarget = stack.querySelector<HTMLElement>(
    ".thumbnail-collapsed-hit-target:not(:disabled)",
  );
  if (!hitTarget) return null;
  if (directTarget?.closest(".thumbnail-collapsed-hit-target") === hitTarget) {
    return hitTarget;
  }
  if (containsPoint(hitTarget, x, y)) return hitTarget;
  // Peeking stacked cards sit above the front-card rect. Treat their paint
  // bounds as the same pile action so the stack never shows a file-drag grab
  // on a decorative card image.
  for (const card of stack.querySelectorAll<HTMLElement>(":scope > .thumbnail-card")) {
    if (containsPoint(card, x, y)) return hitTarget;
  }
  return null;
}

function thumbnailStackControlAtPoint(
  x: number,
  y: number,
  directTarget: Element | null,
  root: Document = document,
): HTMLElement | null {
  const expandControl = minimizedStackExpandControlAtPoint(x, y, directTarget, root);
  if (expandControl) return expandControl;

  const directControl = directTarget?.closest<HTMLElement>(THUMBNAIL_STACK_CONTROL_SELECTOR);
  if (directControl) return directControl;

  const controls = root.querySelectorAll<HTMLElement>(THUMBNAIL_STACK_CONTROL_SELECTOR);
  for (const control of controls) {
    if (containsPoint(control, x, y)) return control;
  }
  return null;
}

/**
 * Activates the hovered preview card and returns which cursor to show.
 *
 * - `pointer` over action buttons, stack chrome, and the collapsed pile
 *   (click expands; a drag still uses the grabbing cursor)
 * - `grab` over the preview image / card chrome (file drag source)
 * - `default` outside a live card
 */
export function applyThumbnailNativeHover(
  position: ThumbnailPointerPosition,
  root: Document = document,
): ThumbnailCursorKind {
  if (!position.inside) {
    root.querySelectorAll<HTMLElement>(THUMBNAIL_EDITOR_JUST_OPENED_SELECTOR)
      .forEach((control) => rearmThumbnailEditorControlHover(control, { fromLeave: true }));
    clearThumbnailNativeHover(root);
    return "default";
  }

  // React pointerleave covers Windows/Linux and an active WebView. The native
  // poll owns this re-arm path for a non-key macOS preview, where WebKit may not
  // dispatch hover transitions while another application stays active.
  root.querySelectorAll<HTMLElement>(THUMBNAIL_EDITOR_JUST_OPENED_SELECTOR)
    .forEach((control) => {
      if (!containsPoint(control, position.x, position.y)) {
        rearmThumbnailEditorControlHover(control, { fromLeave: true });
      }
    });

  const currentButton = root.querySelector<HTMLElement>(
    THUMBNAIL_NATIVE_POINTER_HOVER_SELECTOR,
  );
  const currentCard = root.querySelector<HTMLElement>(THUMBNAIL_NATIVE_ACTIVE_SELECTOR);
  const directTarget = root.elementFromPoint(position.x, position.y);
  const stackControl = thumbnailStackControlAtPoint(
    position.x,
    position.y,
    directTarget,
    root,
  );
  if (stackControl) {
    const ignoreCollapsedHover = stackControl.classList.contains(
      "thumbnail-collapsed-hit-target",
    ) && Boolean(root.querySelector(THUMBNAIL_STACK_HOVER_LATCHED_SELECTOR));
    root.querySelectorAll(THUMBNAIL_NATIVE_ACTIVE_SELECTOR)
      .forEach((element) => element.removeAttribute(THUMBNAIL_NATIVE_ACTIVE_ATTRIBUTE));
    root.querySelectorAll(THUMBNAIL_NATIVE_POINTER_HOVER_SELECTOR)
      .forEach((element) => {
        if (ignoreCollapsedHover || element !== stackControl) {
          element.removeAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE);
        }
      });
    if (!ignoreCollapsedHover) {
      stackControl.setAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE, "true");
    }
    return "pointer";
  }
  if (thumbnailStackSuppressesCardHover(root)) {
    clearThumbnailNativeHover(root);
    return "default";
  }
  const card = directTarget?.closest(".thumbnail-card")
    ?? (
      currentCard && containsPoint(currentCard, position.x, position.y)
        ? currentCard
        : null
    );
  if (!card || card.classList.contains("thumbnail-exiting")) {
    clearThumbnailNativeHover(root);
    return "default";
  }

  // The action layers do not accept pointer events until their card is active.
  // Activate the card first, then hit-test again so buttons can be detected
  // while the preview window is not the active macOS window.
  setThumbnailNativeActiveCard(card, root);
  const target = root
    .elementFromPoint(position.x, position.y)
    ?.closest("button");
  const directButton = target && card.contains(target) ? target : null;
  // A focus handoff or :active scale can make WebKit report the preview image
  // for one poll even though the pointer has not left the button. Keep the last
  // button while the native coordinates remain within it so the cursor does not
  // flash to the default arrow.
  const button = directButton
    ?? (
      currentButton
      && card.contains(currentButton)
      && containsPoint(currentButton, position.x, position.y)
        ? currentButton
        : null
    );

  root.querySelectorAll(THUMBNAIL_NATIVE_POINTER_HOVER_SELECTOR)
    .forEach((element) => {
      if (element !== button) {
        element.removeAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE);
      }
    });
  if (button) {
    button.setAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE, "true");
  }
  return button ? "pointer" : "grab";
}
