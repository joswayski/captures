import {
  isPreviewFileDropLanding,
  previewFileDropShouldDismiss,
  previewFileDropShouldReject,
} from "./thumbnailFileDrag";

describe("mini-preview file-drop landing", () => {
  it("treats a drop on the preview stack as a rejected self-drop", () => {
    expect(previewFileDropShouldReject("preview_stack")).toBe(true);
    expect(previewFileDropShouldDismiss("Dropped", "preview_stack")).toBe(false);
    expect(previewFileDropShouldDismiss("Cancelled", "preview_stack")).toBe(false);
  });

  it("keeps the preview when the file lands in another Captures window", () => {
    expect(previewFileDropShouldReject("app_window")).toBe(false);
    expect(previewFileDropShouldDismiss("Dropped", "app_window")).toBe(false);
  });

  it("dismisses only after a successful drop outside Captures", () => {
    expect(previewFileDropShouldDismiss("Dropped", "external")).toBe(true);
    expect(previewFileDropShouldDismiss("Cancelled", "external")).toBe(false);
  });

  it("accepts the native landing tags", () => {
    expect(isPreviewFileDropLanding("preview_stack")).toBe(true);
    expect(isPreviewFileDropLanding("keep")).toBe(false);
  });
});
