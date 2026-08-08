/** Per-editor presence of capture artifacts currently used as layers. */
export type EditorLayerPresence = {
  editor_id: string;
  artifact_ids: string[];
};

/**
 * Keep editor-control labels + the mini-preview ring in the leave path this
 * long after presence clears so the reverse morph / box-shadow ease can finish
 * (must match `.thumbnail-editor-leaving` / control transitions in CSS).
 */
export const EDITOR_PRESENCE_LEAVE_MS = 550;

export function reconcileEditorPresence(
  current: ReadonlyMap<string, readonly string[]>,
  next: EditorLayerPresence,
): Map<string, string[]> {
  const result = new Map<string, string[]>();
  current.forEach((ids, editorId) => {
    result.set(editorId, [...ids]);
  });
  if (next.artifact_ids.length === 0) {
    result.delete(next.editor_id);
  } else {
    result.set(next.editor_id, [...next.artifact_ids]);
  }
  return result;
}

/** Union of every artifact still present as a layer in any open editor. */
export function artifactIdsInEditors(
  presence: ReadonlyMap<string, readonly string[]>,
): Set<string> {
  const ids = new Set<string>();
  presence.forEach((list) => {
    list.forEach((id) => ids.add(id));
  });
  return ids;
}

export function sameSortedIds(a: readonly string[], b: readonly string[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((id, index) => id === b[index]);
}
