import { reconcileActiveViewer } from "./viewerActivation";

describe("viewer activation reconciliation", () => {
  it("tracks the artifact displayed by the last focused open viewer", () => {
    expect(reconcileActiveViewer(null, {
      artifact_id: "capture-1",
      active: true,
    })).toBe("capture-1");
  });

  it("moves the highlight when another viewer becomes active", () => {
    expect(reconcileActiveViewer("capture-1", {
      artifact_id: "capture-2",
      active: true,
    })).toBe("capture-2");
  });

  it("clears the highlight when the active viewer closes", () => {
    expect(reconcileActiveViewer("capture-1", {
      artifact_id: "capture-1",
      active: false,
    })).toBeNull();
  });

  it("ignores a close from a viewer that is no longer active", () => {
    expect(reconcileActiveViewer("capture-2", {
      artifact_id: "capture-1",
      active: false,
    })).toBe("capture-2");
  });
});
