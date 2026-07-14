import type { ThumbnailPointerPosition } from "../types";

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
  clearThumbnailNativeHover(root);
  if (!position.inside) return false;

  const card = root
    .elementFromPoint(position.x, position.y)
    ?.closest(".thumbnail-card");
  if (!card) return false;

  // The action layers do not accept pointer events until their card is active.
  // Activate the card first, then hit-test again so buttons can be detected
  // while the preview window is not the active macOS window.
  card.classList.add("thumbnail-card-native-active");
  const button = root
    .elementFromPoint(position.x, position.y)
    ?.closest("button");
  if (!button || !card.contains(button)) return false;

  button.classList.add("native-pointer-hover");
  return true;
}
