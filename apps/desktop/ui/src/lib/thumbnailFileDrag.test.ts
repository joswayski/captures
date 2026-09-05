import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  isPreviewFileDropLanding,
  previewFileDropShouldDismiss,
  previewFileDropShouldReject,
  THUMBNAIL_DROP_REJECT_MS,
} from "./thumbnailFileDrag";

const thumbnailStyles = readFileSync(
  resolve(process.cwd(), "ui/src/styles/mini-preview.css"),
  "utf8",
);
const designTokens = readFileSync(
  resolve(process.cwd(), "../../shared/design.css"),
  "utf8",
);

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

  it("shakes with shared duration and spacing tokens", () => {
    expect(designTokens).toMatch(/--thumbnail-drop-reject-duration:\s*var\(--dur-5\)/);
    expect(designTokens).toMatch(/--thumbnail-drop-reject-ease:\s*linear/);
    expect(designTokens).toMatch(/--thumbnail-drop-reject-x-1:\s*var\(--s-5\)/);
    expect(designTokens).not.toMatch(/--thumbnail-drop-reject-x-5:/);
    expect(THUMBNAIL_DROP_REJECT_MS).toBe(420);
    expect(thumbnailStyles).toMatch(
      /animation:\s*thumbnail-drop-reject var\(--thumbnail-drop-reject-duration\)\s+var\(--thumbnail-drop-reject-ease\)/,
    );
    expect(thumbnailStyles).toMatch(
      /translate:\s*calc\(-1 \* var\(--thumbnail-drop-reject-x-1\)\) 0/,
    );
    expect(thumbnailStyles).toMatch(
      /translate:\s*var\(--thumbnail-drop-reject-x-1\) 0/,
    );
    expect(thumbnailStyles).not.toMatch(/translate:\s*-9px 0/);
  });

  it("lets the reject shake override the settled arrive animation", () => {
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-card\.thumbnail-ready\.thumbnail-arrived:not\(\.thumbnail-exiting\):not\(\.thumbnail-drop-rejected\)/,
    );
    expect(thumbnailStyles).toMatch(
      /\.thumbnail-card\.thumbnail-ready\.thumbnail-arrived\.thumbnail-drop-rejected/,
    );
  });
});

