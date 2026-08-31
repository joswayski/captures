import type { ThumbnailPointerPosition } from "../types";
import {
  buildThumbnailDustParticles,
  type ThumbnailDustParticle,
} from "./thumbnailExit";

/** Copy for the parked mini-preview restore chip. */
export function miniPreviewsHiddenLabel(count: number): string {
  if (count <= 0) return "Previews";
  if (count === 1) return "1 preview";
  return `${count} previews`;
}

/** Full card-to-folder motion, including open-folder lead-in and stagger. */
export const MINI_PREVIEW_FOLDER_MORPH_MS = 720;

/** Compact hide control before the isometric folder grows to the right. */
export const MINI_PREVIEW_FOLDER_IDLE_WIDTH = 48;
export const MINI_PREVIEW_FOLDER_IDLE_HEIGHT = 48;

/**
 * Grown open-folder control. Hide, parked, and restore all share this size so
 * the glass never collapses and re-extends around the pocket.
 */
export const MINI_PREVIEW_FOLDER_OPEN_WIDTH = 80;
export const MINI_PREVIEW_FOLDER_OPEN_HEIGHT = 56;

/** Pocket origin inside the grown control, from its stable bottom-left corner. */
export const MINI_PREVIEW_FOLDER_POCKET_INSET_LEFT = 20;
export const MINI_PREVIEW_FOLDER_POCKET_INSET_TOP = 10;
export const MINI_PREVIEW_FOLDER_SHEET_WIDTH = 22;
export const MINI_PREVIEW_FOLDER_SHEET_HEIGHT = 26;

/** Compact folder control used when the last preview dissolves the hide button. */
export const MINI_PREVIEW_FOLDER_SIZE_PX = MINI_PREVIEW_FOLDER_IDLE_WIDTH;

/**
 * Hold the folder together until the outgoing card's ash front reaches it.
 * 50ms earlier than the previous 420ms hold so the icon starts breaking as
 * the preview wave blows past, instead of sitting fully formed.
 */
export const MINI_PREVIEW_FOLDER_DUST_LEAD_MS = 370;

/** A shorter copy of the card dissolve wave, scaled to the compact control. */
export const MINI_PREVIEW_FOLDER_DUST_WAVE_MS = 420;

/**
 * Slice the complete folder control into real chips using the same breakup and
 * cross-platform WAAPI flight as a deleted mini preview. The wave still begins
 * at the folder's top-right card contact point instead of the trash button.
 */
export function buildMiniPreviewFolderDustParticles(
  random: () => number = Math.random,
): ThumbnailDustParticle[] {
  return buildThumbnailDustParticles(
    MINI_PREVIEW_FOLDER_SIZE_PX,
    MINI_PREVIEW_FOLDER_SIZE_PX,
    {
      cols: 6,
      rows: 6,
      random,
      waveMs: MINI_PREVIEW_FOLDER_DUST_WAVE_MS,
      originX: MINI_PREVIEW_FOLDER_SIZE_PX,
      originY: 0,
      chromeLeadMs: MINI_PREVIEW_FOLDER_DUST_LEAD_MS,
    },
  );
}

/** DOM event emitted by the native restore command once the stack is onscreen. */
export const MINI_PREVIEWS_RESTORED_EVENT = "captures-mini-previews-restored";

/** Newest captures peek out of the 3D folder; older ones stay stacked behind. */
export const MINI_PREVIEW_FOLDER_SHEET_LIMIT = 4;

export type MiniPreviewFolderSheet = {
  id: string;
  src: string | null;
};

export type MiniPreviewFolderPose = "idle" | "open" | "parked";

let pendingMiniPreviewRestore = false;

/** Native restore may remount the stack; start that mount already in the open pose. */
export function markMiniPreviewRestorePending(): void {
  pendingMiniPreviewRestore = true;
}

export function takeMiniPreviewRestorePending(): boolean {
  const pending = pendingMiniPreviewRestore;
  pendingMiniPreviewRestore = false;
  return pending;
}

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
 * The restore chip sits in a larger transparent window so its shadow can fade
 * out. Empty gutter, and the restoring pose, must pass clicks through
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
 * Measures each live card against the grown folder pocket and stores the travel
 * as CSS variables. The open control grows right from a stable bottom-left, so
 * travel uses that parked size even while the idle 48px control is still onscreen.
 */
export function prepareMiniPreviewFolderMotion(
  stack: ParentNode,
  folder: Element,
): number {
  const cards = Array.from(
    stack.querySelectorAll<HTMLElement>(".thumbnail-card:not(.thumbnail-exiting)"),
  );
  const host = folder.closest(".thumbnail-collapse, .mini-previews-hidden") ?? folder;
  const hostBounds = host.getBoundingClientRect();
  const openLeft = hostBounds.left;
  const openTop = hostBounds.bottom - MINI_PREVIEW_FOLDER_OPEN_HEIGHT;

  cards.forEach((card, index) => {
    const cardBounds = card.getBoundingClientRect();
    const width = Math.max(1, cardBounds.width);
    const height = Math.max(1, cardBounds.height);
    // Sheet-sized miniatures land in the open pocket, then the front flap
    // hides them. Keep the card's aspect ratio so a tall capture still reads.
    const scale = Math.min(
      MINI_PREVIEW_FOLDER_SHEET_WIDTH / width,
      MINI_PREVIEW_FOLDER_SHEET_HEIGHT / height,
    );
    const newestFirst = cards.length - 1 - index;
    const depth = Math.min(newestFirst, 3);
    const targetLeft = openLeft + MINI_PREVIEW_FOLDER_POCKET_INSET_LEFT - depth * 2.2;
    const targetTop = openTop + MINI_PREVIEW_FOLDER_POCKET_INSET_TOP - depth * 2.8;

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
