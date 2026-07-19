import { reconcileClipboardState } from "./clipboard";

describe("clipboard state reconciliation", () => {
  it("clears the owner when a newer system clipboard revision has no capture owned by Captures", () => {
    expect(reconcileClipboardState(
      { revision: 41, artifact_id: "capture-1" },
      { revision: 42, artifact_id: null },
    )).toEqual({ revision: 42, artifact_id: null });
  });

  it("ignores a stale poll that completes after a newer clipboard event", () => {
    expect(reconcileClipboardState(
      { revision: 42, artifact_id: "capture-2" },
      { revision: 41, artifact_id: null },
    )).toEqual({ revision: 42, artifact_id: "capture-2" });
  });

  it("prefers a known owner when an event and poll share a revision", () => {
    expect(reconcileClipboardState(
      { revision: 42, artifact_id: null },
      { revision: 42, artifact_id: "capture-2" },
    )).toEqual({ revision: 42, artifact_id: "capture-2" });
    expect(reconcileClipboardState(
      { revision: 42, artifact_id: "capture-2" },
      { revision: 42, artifact_id: null },
    )).toEqual({ revision: 42, artifact_id: "capture-2" });
  });
});
