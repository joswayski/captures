import type { ThumbnailPointerPosition } from "../types";

export const THUMBNAIL_CURSOR_REASSERT_INTERVAL_MS = 100;
const THUMBNAIL_NATIVE_ACTIVE_ATTRIBUTE = "data-thumbnail-native-active";
const THUMBNAIL_NATIVE_ACTIVE_SELECTOR = `[${THUMBNAIL_NATIVE_ACTIVE_ATTRIBUTE}="true"]`;

export function thumbnailCursorSyncAction(
  current: boolean,
  next: boolean,
  elapsedMs: number,
): "transition" | "reassert" | null {
  if (current !== next) return "transition";
  if (next && elapsedMs >= THUMBNAIL_CURSOR_REASSERT_INTERVAL_MS) return "reassert";
  return null;
}

/**
 * Keep the native window interactive only over a live preview card. After a
 * dismiss it may stay tall (shrinking blanks WKWebView), and a deleting card
 * keeps its layout slot while its particles finish. Empty space and exiting
 * slots must pass clicks through without disabling the remaining cards.
 */
export function shouldIgnoreThumbnailCursorEvents(
  position: ThumbnailPointerPosition,
  root: Document = document,
): boolean {
  if (!position.inside) return false;
  const target = root.elementFromPoint(position.x, position.y);
  if (!target) return true;
  const card = target.closest(".thumbnail-card");
  return !card || card.classList.contains("thumbnail-exiting");
}

export function clearThumbnailNativeHover(root: ParentNode = document) {
  root.querySelectorAll(`${THUMBNAIL_NATIVE_ACTIVE_SELECTOR}, .native-pointer-hover`)
    .forEach((element) => {
      element.removeAttribute(THUMBNAIL_NATIVE_ACTIVE_ATTRIBUTE);
      element.classList.remove("native-pointer-hover");
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

export function applyThumbnailNativeHover(
  position: ThumbnailPointerPosition,
  root: Document = document,
): boolean {
  if (!position.inside) {
    clearThumbnailNativeHover(root);
    return false;
  }

  const currentButton = root.querySelector<HTMLElement>(".native-pointer-hover");
  const currentCard = root.querySelector<HTMLElement>(THUMBNAIL_NATIVE_ACTIVE_SELECTOR);
  const directTarget = root.elementFromPoint(position.x, position.y);
  const card = directTarget?.closest(".thumbnail-card")
    ?? (
      currentCard && containsPoint(currentCard, position.x, position.y)
        ? currentCard
        : null
    );
  if (!card) {
    clearThumbnailNativeHover(root);
    return false;
  }

  // The action layers do not accept pointer events until their card is active.
  // Activate the card first, then hit-test again so buttons can be detected
  // while the preview window is not the active macOS window.
  setThumbnailNativeActiveCard(card, root);
  const target = root
    .elementFromPoint(position.x, position.y)
    ?.closest("button");
  const directButton = target && card.contains(target) ? target : null;
  // A focus handoff can make WebKit report the preview image for one poll even
  // though the pointer has not left the button. Keep the last button while the
  // native coordinates remain within it so the cursor does not flash to arrow.
  const button = directButton
    ?? (
      currentButton
      && card.contains(currentButton)
      && containsPoint(currentButton, position.x, position.y)
        ? currentButton
        : null
    );

  root.querySelectorAll(".native-pointer-hover")
    .forEach((element) => {
      if (element !== button) element.classList.remove("native-pointer-hover");
    });
  button?.classList.add("native-pointer-hover");
  return button !== null;
}
