import {
  artifactIdsInEditors,
  reconcileEditorPresence,
  sameSortedIds,
} from "./editorPresence";

describe("editor presence reconciliation", () => {
  it("records layers for an open editor", () => {
    const next = reconcileEditorPresence(new Map(), {
      editor_id: "screenshot-editor-a",
      artifact_ids: ["capture-1"],
    });
    expect([...artifactIdsInEditors(next)]).toEqual(["capture-1"]);
  });

  it("unions artifacts across multiple editors", () => {
    let presence = reconcileEditorPresence(new Map(), {
      editor_id: "screenshot-editor-a",
      artifact_ids: ["capture-1", "capture-2"],
    });
    presence = reconcileEditorPresence(presence, {
      editor_id: "recording-editor-b",
      artifact_ids: ["capture-3"],
    });
    expect(artifactIdsInEditors(presence)).toEqual(
      new Set(["capture-1", "capture-2", "capture-3"]),
    );
  });

  it("clears an editor when it reports no remaining layers", () => {
    let presence = reconcileEditorPresence(new Map(), {
      editor_id: "screenshot-editor-a",
      artifact_ids: ["capture-1", "capture-2"],
    });
    presence = reconcileEditorPresence(presence, {
      editor_id: "screenshot-editor-a",
      artifact_ids: ["capture-2"],
    });
    expect(artifactIdsInEditors(presence)).toEqual(new Set(["capture-2"]));

    presence = reconcileEditorPresence(presence, {
      editor_id: "screenshot-editor-a",
      artifact_ids: [],
    });
    expect(artifactIdsInEditors(presence).size).toBe(0);
  });

  it("leaves other editors alone when one closes", () => {
    let presence = reconcileEditorPresence(new Map(), {
      editor_id: "screenshot-editor-a",
      artifact_ids: ["capture-1"],
    });
    presence = reconcileEditorPresence(presence, {
      editor_id: "screenshot-editor-b",
      artifact_ids: ["capture-2"],
    });
    presence = reconcileEditorPresence(presence, {
      editor_id: "screenshot-editor-a",
      artifact_ids: [],
    });
    expect(artifactIdsInEditors(presence)).toEqual(new Set(["capture-2"]));
  });

  it("compares sorted id lists for emit dedupe", () => {
    expect(sameSortedIds(["a", "b"], ["a", "b"])).toBe(true);
    expect(sameSortedIds(["a"], ["a", "b"])).toBe(false);
    expect(sameSortedIds(["a", "b"], ["b", "a"])).toBe(false);
  });
});
