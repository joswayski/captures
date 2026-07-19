import type { ViewerActivationState } from "../types";

export function reconcileActiveViewer(
  currentArtifactId: string | null,
  next: ViewerActivationState,
): string | null {
  if (next.active) return next.artifact_id;
  return currentArtifactId === next.artifact_id ? null : currentArtifactId;
}
