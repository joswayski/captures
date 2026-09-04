import type { PreviewFileDropLanding } from "../types";

export type { PreviewFileDropLanding };

/** macOS-style “no” shake when a preview is dropped back on itself. */
export const THUMBNAIL_DROP_REJECT_ANIMATION = "thumbnail-drop-reject";

export const THUMBNAIL_DROP_REJECT_MS = 420; // matches `--dur-5` / `--thumbnail-drop-reject-duration`

export function isPreviewFileDropLanding(value: unknown): value is PreviewFileDropLanding {
  return value === "preview_stack" || value === "app_window" || value === "external";
}

/** Dropping onto the source stack is invalid; keep the card and shake. */
export function previewFileDropShouldReject(
  landing: PreviewFileDropLanding,
): boolean {
  return landing === "preview_stack";
}

/**
 * Only a drop outside Captures (Finder, Slack, a browser, the desktop)
 * dismisses the mini preview. Self-drops and in-app drops stay put.
 */
export function previewFileDropShouldDismiss(
  result: "Dropped" | "Cancelled",
  landing: PreviewFileDropLanding,
): boolean {
  return result === "Dropped" && landing === "external";
}
