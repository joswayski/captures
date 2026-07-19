import type { ClipboardState } from "../types";

export function reconcileClipboardState(
  current: ClipboardState,
  next: ClipboardState,
): ClipboardState {
  if (next.revision > current.revision) return next;
  if (
    next.revision === current.revision
    && next.artifact_id
    && next.artifact_id !== current.artifact_id
  ) return next;
  return current;
}
