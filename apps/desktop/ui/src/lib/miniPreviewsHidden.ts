import type { ThumbnailPointerPosition } from "../types";

/** Copy for the parked mini-preview restore chip. */
export function miniPreviewsHiddenLabel(count: number): string {
  if (count <= 0) return "Previews";
  if (count === 1) return "1 preview";
  return `${count} previews`;
}

/** Full card-to-folder motion, including the short per-card rollout stagger. */
export const MINI_PREVIEW_FOLDER_MORPH_MS = 640;

/** Time for the parked slab to fold back to the shared bottom-left anchor. */
export const MINI_PREVIEW_FOLDER_RESTORE_LEAD_MS = 260;

/** DOM event emitted by the native restore command once the stack is onscreen. */
export const MINI_PREVIEWS_RESTORED_EVENT = "captures-mini-previews-restored";

/**
 * The restore chip sits in a larger transparent window so its shadow can fade
 * out. Empty gutter, and the shrinking restore pose, must pass clicks through
 * to the desktop underneath.
 */
export function shouldIgnoreMiniPreviewsHiddenCursorEvents(
  position: ThumbnailPointerPosition,
  root: Document = document,
): boolean {
  const chip = root.querySelector(".mini-previews-hidden");
  if (!chip || chip.classList.contains("mini-previews-hidden-restoring")) {
    return true;
  }
  if (!position.inside) return false;
  return !elementContainsPoint(chip, position.x, position.y);
}

function elementContainsPoint(element: Element, x: number, y: number): boolean {
  const bounds = element.getBoundingClientRect();
  return x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom;
}

/**
 * Measures each live card against the shared folder icon and stores the travel
 * as CSS variables. Measurement keeps the motion correct when the stack is
 * scrolled or a monitor can show only part of it.
 */
export function prepareMiniPreviewFolderMotion(
  stack: ParentNode,
  folder: Element,
): number {
  const cards = Array.from(
    stack.querySelectorAll<HTMLElement>(".thumbnail-card:not(.thumbnail-exiting)"),
  );
  const folderBounds = folder.getBoundingClientRect();

  cards.forEach((card, index) => {
    const cardBounds = card.getBoundingClientRect();
    const width = Math.max(1, cardBounds.width);
    const height = Math.max(1, cardBounds.height);
    // The miniature keeps the card's aspect ratio and peeks above the folder
    // face before disappearing behind it.
    const scale = Math.min(30 / width, 18 / height);
    const newestFirst = cards.length - 1 - index;
    const depth = Math.min(newestFirst, 3);
    const targetLeft = folderBounds.left + 1 - depth * 1.5;
    const targetTop = folderBounds.top - 1 - depth * 1.5;

    card.style.setProperty(
      "--thumbnail-folder-x",
      `${targetLeft - cardBounds.left}px`,
    );
    card.style.setProperty(
      "--thumbnail-folder-y",
      `${targetTop - cardBounds.top}px`,
    );
    card.style.setProperty("--thumbnail-folder-scale", `${scale}`);
    card.style.setProperty(
      "--thumbnail-folder-delay",
      `${Math.min(newestFirst, 5) * 24}ms`,
    );
  });

  return cards.length;
}
