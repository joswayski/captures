import type { ThumbnailPointerPosition } from "../types";

export const THUMBNAIL_CURSOR_REASSERT_INTERVAL_MS = 100;

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
  root.querySelectorAll(".thumbnail-card-native-active, .native-pointer-hover")
    .forEach((element) => {
      element.classList.remove("thumbnail-card-native-active", "native-pointer-hover");
    });
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
  root.querySelectorAll(".thumbnail-card-native-active")
    .forEach((element) => {
      if (element !== card) element.classList.remove("thumbnail-card-native-active");
    });
  card.classList.add("thumbnail-card-native-active");
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
