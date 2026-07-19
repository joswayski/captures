import { render, screen, within } from "@testing-library/react";

import { ThumbnailCard } from "./App";
import type { CaptureArtifact } from "./types";

function artifact(path: string | null, id = "capture-1"): CaptureArtifact {
  return {
    id,
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
    render(<ThumbnailCard artifact={artifact(null)} clipboardCurrent onRemoved={() => undefined} />);

    expect(screen.getByText("Copied to clipboard")).toBeInTheDocument();
    const fullSize = screen.getByRole("button", { name: "View Full Size" });
    const close = screen.getByRole("button", { name: "Close Without Saving" });
    expect(fullSize.parentElement).toHaveClass("thumbnail-top-right");
    expect(close.parentElement).toHaveClass("thumbnail-top-left");
    expect(screen.queryByRole("button", { name: "Move to Trash" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Copy" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeInTheDocument();
  });

  it("offers deletion only after the capture has a saved file", () => {
    render(<ThumbnailCard artifact={artifact("/Users/josevalerio/CES/capture.png")} clipboardCurrent onRemoved={() => undefined} />);

    const close = screen.getByRole("button", { name: "Close Preview" });
    const trash = screen.getByRole("button", { name: "Move to Trash" });
    expect(close.parentElement).toBe(trash.parentElement);
    expect(close.parentElement).toHaveClass("thumbnail-top-left");
    expect(screen.getByRole("button", { name: "Show in Folder" })).toBeInTheDocument();
  });

  it("does not claim the clipboard changed when automatic copying is disabled", () => {
    const notCopied = { ...artifact(null), clipboard_copy_status: "skipped" as const };
    render(<ThumbnailCard artifact={notCopied} clipboardCurrent={false} onRemoved={() => undefined} />);

    expect(screen.queryByText("Copied to clipboard")).not.toBeInTheDocument();
    expect(screen.queryByText("Clipboard unavailable")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
  });

  it("reports an automatic clipboard failure without showing a success confirmation", () => {
    const failedCopy = { ...artifact(null), clipboard_copy_status: "failed" as const };
    render(<ThumbnailCard artifact={failedCopy} clipboardCurrent={false} onRemoved={() => undefined} />);

    expect(screen.queryByText("Copied to clipboard")).not.toBeInTheDocument();
    expect(screen.getByText("Clipboard unavailable")).toBeInTheDocument();
  });

  it("hides Copy only on the preview that currently owns the clipboard", () => {
    render(
      <>
        <ThumbnailCard artifact={artifact(null, "older")} clipboardCurrent={false} onRemoved={() => undefined} />
        <ThumbnailCard artifact={artifact(null, "current")} clipboardCurrent onRemoved={() => undefined} />
      </>,
    );

    const [older, current] = screen.getAllByRole("article");
    expect(within(older).getByRole("button", { name: "Copy" })).toBeInTheDocument();
    expect(within(older).queryByText("Copied to clipboard")).not.toBeInTheDocument();
    expect(within(current).queryByRole("button", { name: "Copy" })).not.toBeInTheDocument();
    expect(within(current).getByText("Copied to clipboard")).toBeInTheDocument();
  });
});
