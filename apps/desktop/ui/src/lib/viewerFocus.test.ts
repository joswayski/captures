import { reconcileFocusedViewer } from "./viewerFocus";

describe("viewer focus reconciliation", () => {
  it("tracks the artifact displayed by the focused viewer", () => {
    expect(reconcileFocusedViewer(null, {
      artifact_id: "capture-1",
      focused: true,
    })).toBe("capture-1");
  });

  it("clears the highlight when the current viewer loses focus", () => {
    expect(reconcileFocusedViewer("capture-1", {
      artifact_id: "capture-1",
      focused: false,
    })).toBeNull();
  });

  it("ignores a late blur from a viewer that is no longer current", () => {
    expect(reconcileFocusedViewer("capture-2", {
      artifact_id: "capture-1",
      focused: false,
    })).toBe("capture-2");
  });
});
