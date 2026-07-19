import type { ViewerFocusState } from "../types";

export function reconcileFocusedViewer(
  currentArtifactId: string | null,
  next: ViewerFocusState,
): string | null {
  if (next.focused) return next.artifact_id;
  return currentArtifactId === next.artifact_id ? null : currentArtifactId;
}
