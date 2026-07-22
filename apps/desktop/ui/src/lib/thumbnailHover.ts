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
 * After a dismiss the native window may stay tall (shrinking blanks WKWebView).
 * Empty space above the bottom-anchored stack must not steal clicks.
 */
export function shouldIgnoreThumbnailCursorEvents(
  position: ThumbnailPointerPosition,
  root: Document = document,
): boolean {
  if (!position.inside) return false;
  const target = root.elementFromPoint(position.x, position.y);
  if (!target) return true;
  return !target.closest(".thumbnail-stack");
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

export function applyThumbnailNativeHover(
  position: ThumbnailPointerPosition,
  root: Document = document,
): boolean {
  if (!position.inside) {
    clearThumbnailNativeHover(root);
    return false;
  }

  const card = root
    .elementFromPoint(position.x, position.y)
    ?.closest(".thumbnail-card");
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
  const button = target && card.contains(target) ? target : null;

  root.querySelectorAll(".native-pointer-hover")
    .forEach((element) => {
      if (element !== button) element.classList.remove("native-pointer-hover");
    });
  button?.classList.add("native-pointer-hover");
  return button !== null;
}
