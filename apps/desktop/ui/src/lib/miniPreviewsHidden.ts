/** Copy for the parked mini-preview restore chip. */
export function miniPreviewsHiddenLabel(count: number): string {
  if (count <= 0) return "Previews";
  if (count === 1) return "1 preview";
  return `${count} previews`;
}

/** Full card-to-folder motion, including the short per-card rollout stagger. */
export const MINI_PREVIEW_FOLDER_MORPH_MS = 640;

/** DOM event emitted by the native restore command once the stack is onscreen. */
export const MINI_PREVIEWS_RESTORED_EVENT = "captures-mini-previews-restored";

/** Newest captures peek out of the 3D folder; older ones stay stacked behind. */
export const MINI_PREVIEW_FOLDER_SHEET_LIMIT = 4;

export type MiniPreviewFolderSheet = {
  id: string;
  src: string | null;
};

export type MiniPreviewFolderPose = "idle" | "open" | "parked";

/** Placeholder sheets so the parked folder has volume before preview URLs load. */
export function miniPreviewFolderPlaceholderSheets(
  count: number,
  limit = MINI_PREVIEW_FOLDER_SHEET_LIMIT,
): MiniPreviewFolderSheet[] {
  const sheetCount = Math.min(Math.max(count, 0), limit);
  return Array.from({ length: sheetCount }, (_, index) => ({
    id: `preview-sheet-${index}`,
    src: null,
  }));
}

/** Newest-first sheets that stand in the open folder pocket. */
export function miniPreviewFolderSheets(
  artifacts: Array<{ id: string; preview_url: string | null }>,
  limit = MINI_PREVIEW_FOLDER_SHEET_LIMIT,
): MiniPreviewFolderSheet[] {
  return artifacts
    .slice(-limit)
    .reverse()
    .map((artifact) => ({
      id: artifact.id,
      src: artifact.preview_url,
    }));
}

/**
 * Measures each live card against the shared folder pocket and stores the travel
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
    // Sheet-sized miniatures land in the open pocket, then the 3D flaps hide
    // them. Keep the card's aspect ratio so a tall capture still reads as itself.
    const scale = Math.min(22 / width, 16 / height);
    const newestFirst = cards.length - 1 - index;
    const depth = Math.min(newestFirst, 3);
    const targetLeft = folderBounds.left + folderBounds.width * 0.22 - depth * 2.2;
    const targetTop = folderBounds.top + folderBounds.height * 0.08 - depth * 2.8;

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
