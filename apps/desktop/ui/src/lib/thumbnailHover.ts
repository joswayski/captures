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
