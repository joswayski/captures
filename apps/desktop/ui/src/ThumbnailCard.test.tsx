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
    clipboard_copied: true,
  };
}

describe("ThumbnailCard", () => {
  it("explains automatic clipboard copying and closes an unsaved preview without a trash action", () => {
    render(<ThumbnailCard artifact={artifact(null)} onRemoved={() => undefined} />);

    expect(screen.getByText("Copied to clipboard")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Close Preview" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete Saved Capture" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeInTheDocument();
  });

  it("offers deletion only after the capture has a saved file", () => {
    render(<ThumbnailCard artifact={artifact("/Users/josevalerio/CES/capture.png")} onRemoved={() => undefined} />);

    expect(screen.getByRole("button", { name: "Delete Saved Capture" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Close Preview" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show in Folder" })).toBeInTheDocument();
  });
});
