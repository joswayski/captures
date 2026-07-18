import { render, screen } from "@testing-library/react";

import { ThumbnailCard } from "./App";
import type { CaptureArtifact } from "./types";

function artifact(path: string | null): CaptureArtifact {
  return {
    id: "capture-1",
    path,
    preview_url: "ces-capture://artifact/capture-1",
    full_url: "ces-capture://artifact-full/capture-1",
    width: 1_440,
    height: 900,
    size_bytes: 250_000,
    created_at: "2026-07-18T22:00:00Z",
    mode: "region",
    clipboard_copy_status: "copied",
  };
}

describe("ThumbnailCard", () => {
  it("explains automatic clipboard copying and closes an unsaved preview without a trash action", () => {
    render(<ThumbnailCard artifact={artifact(null)} onRemoved={() => undefined} />);

    expect(screen.getByText("Copied to clipboard")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "View Full Size" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Close Without Saving" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Move to Trash" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeInTheDocument();
  });

  it("offers deletion only after the capture has a saved file", () => {
    render(<ThumbnailCard artifact={artifact("/Users/josevalerio/CES/capture.png")} onRemoved={() => undefined} />);

    expect(screen.getByRole("button", { name: "Move to Trash" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Close Preview" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show in Folder" })).toBeInTheDocument();
  });

  it("does not claim the clipboard changed when automatic copying is disabled", () => {
    const notCopied = { ...artifact(null), clipboard_copy_status: "skipped" as const };
    render(<ThumbnailCard artifact={notCopied} onRemoved={() => undefined} />);

    expect(screen.queryByText("Copied to clipboard")).not.toBeInTheDocument();
    expect(screen.queryByText("Clipboard unavailable")).not.toBeInTheDocument();
  });

  it("reports an automatic clipboard failure without showing a success confirmation", () => {
    const failedCopy = { ...artifact(null), clipboard_copy_status: "failed" as const };
    render(<ThumbnailCard artifact={failedCopy} onRemoved={() => undefined} />);

    expect(screen.queryByText("Copied to clipboard")).not.toBeInTheDocument();
    expect(screen.getByText("Clipboard unavailable")).toBeInTheDocument();
  });
});
