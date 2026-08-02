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
 * key-window steal when Edit opens the screenshot editor.
 */
export const THUMBNAIL_CURSOR_HANDOFF_REASSERT_DELAYS_MS = [0, 16, 48] as const;

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
 * Keep the native window interactive only over a live preview card or stack
 * overflow control. After a dismiss it may stay tall (shrinking blanks
 * WKWebView), and a deleting card keeps its layout slot while its particles
 * finish. Empty space and exiting slots must pass clicks through without
 * disabling the remaining cards.
 */
export function shouldIgnoreThumbnailCursorEvents(
  position: ThumbnailPointerPosition,
  root: Document = document,
): boolean {
  if (!position.inside) return false;
  const target = root.elementFromPoint(position.x, position.y);
  if (!target) return true;
  if (target.closest(".thumbnail-overflow-cue")) return false;
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

function containsPoint(element: Element, x: number, y: number): boolean {
  const bounds = element.getBoundingClientRect();
  return x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom;
}

/**
 * Activates the hovered preview card and returns which cursor to show.
 *
 * - `pointer` over action buttons
 * - `grab` over the preview image / card chrome (file drag source)
 * - `default` outside a live card
 */
export function applyThumbnailNativeHover(
  position: ThumbnailPointerPosition,
  root: Document = document,
): ThumbnailCursorKind {
  if (!position.inside) {
    clearThumbnailNativeHover(root);
    return "default";
  }

  const currentButton = root.querySelector<HTMLElement>(
    THUMBNAIL_NATIVE_POINTER_HOVER_SELECTOR,
  );
  const currentCard = root.querySelector<HTMLElement>(THUMBNAIL_NATIVE_ACTIVE_SELECTOR);
  const directTarget = root.elementFromPoint(position.x, position.y);
  const directOverflowCue = directTarget?.closest(".thumbnail-overflow-cue");
  const overflowCue = directOverflowCue
    ?? (
      currentButton?.classList.contains("thumbnail-overflow-cue")
      && containsPoint(currentButton, position.x, position.y)
        ? currentButton
        : null
    );
  if (overflowCue) {
    root.querySelectorAll(THUMBNAIL_NATIVE_ACTIVE_SELECTOR)
      .forEach((element) => element.removeAttribute(THUMBNAIL_NATIVE_ACTIVE_ATTRIBUTE));
    root.querySelectorAll(THUMBNAIL_NATIVE_POINTER_HOVER_SELECTOR)
      .forEach((element) => {
        if (element !== overflowCue) {
          element.removeAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE);
        }
      });
    overflowCue.setAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE, "true");
    return "pointer";
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
