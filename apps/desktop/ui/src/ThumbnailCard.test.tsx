import { invoke } from "@tauri-apps/api/core";
import { act, render, screen, within } from "@testing-library/react";

import { ThumbnailCard } from "./App";
import type { CaptureArtifact } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => undefined),
  isTauri: () => false,
}));

function artifact(path: string | null, id = "capture-1"): CaptureArtifact {
  return {
    id,
    path,
    preview_url: "captures-capture://artifact/capture-1",
    full_url: "captures-capture://artifact-full/capture-1",
    width: 1_440,
    height: 900,
    size_bytes: 250_000,
    created_at: "2026-07-18T22:00:00Z",
    mode: "region",
    history_saved: true,
    clipboard_copy_status: "copied",
  };
}

describe("ThumbnailCard", () => {
  it("explains automatic clipboard copying and keeps a dismissed preview in history", () => {
    render(<ThumbnailCard artifact={artifact(null)} clipboardCurrent viewerActive={false} onRemoved={() => undefined} />);

    expect(screen.getByText("Copied to clipboard")).toBeInTheDocument();
    const fullSize = screen.getByRole("button", { name: "View Full Size" });
    const close = screen.getByRole("button", { name: "Dismiss — available in History for 30 days" });
    expect(fullSize.parentElement).toHaveClass("thumbnail-top-right");
    expect(close.parentElement).toHaveClass("thumbnail-top-left");
    expect(screen.queryByRole("button", { name: "Move to Trash" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Copy" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeInTheDocument();
  });

  it("offers deletion only after the capture has a saved file", () => {
    render(<ThumbnailCard artifact={artifact("/Users/josevalerio/Captures/capture.png")} clipboardCurrent viewerActive={false} onRemoved={() => undefined} />);

    const close = screen.getByRole("button", { name: "Dismiss — available in History for 30 days" });
    const trash = screen.getByRole("button", { name: "Move to Trash" });
    expect(close.parentElement).toBe(trash.parentElement);
    expect(close.parentElement).toHaveClass("thumbnail-top-left");
    expect(screen.getByRole("button", { name: "Show in Folder" })).toBeInTheDocument();
  });

  it("does not promise recovery when local history could not be written", () => {
    render(
      <ThumbnailCard
        artifact={{ ...artifact(null), history_saved: false }}
        clipboardCurrent={false}
        viewerActive={false}
        onRemoved={() => undefined}
      />,
    );

    expect(screen.getByRole("button", { name: "Close Without Saving" })).toBeInTheDocument();
    expect(screen.getByText("History unavailable")).toBeInTheDocument();
  });

  it("does not claim the clipboard changed when automatic copying is disabled", () => {
    const notCopied = { ...artifact(null), clipboard_copy_status: "skipped" as const };
    render(<ThumbnailCard artifact={notCopied} clipboardCurrent={false} viewerActive={false} onRemoved={() => undefined} />);

    expect(screen.queryByText("Copied to clipboard")).not.toBeInTheDocument();
    expect(screen.queryByText("Clipboard unavailable")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy" })).toBeInTheDocument();
  });

  it("reports an automatic clipboard failure without showing a success confirmation", () => {
    const failedCopy = { ...artifact(null), clipboard_copy_status: "failed" as const };
    render(<ThumbnailCard artifact={failedCopy} clipboardCurrent={false} viewerActive={false} onRemoved={() => undefined} />);

    expect(screen.queryByText("Copied to clipboard")).not.toBeInTheDocument();
    expect(screen.getByText("Clipboard unavailable")).toBeInTheDocument();
  });

  it("hides Copy only on the preview that currently owns the clipboard", () => {
    render(
      <>
        <ThumbnailCard artifact={artifact(null, "older")} clipboardCurrent={false} viewerActive={false} onRemoved={() => undefined} />
        <ThumbnailCard artifact={artifact(null, "current")} clipboardCurrent viewerActive={false} onRemoved={() => undefined} />
      </>,
    );

    const [older, current] = screen.getAllByRole("article");
    expect(within(older).getByRole("button", { name: "Copy" })).toBeInTheDocument();
    expect(within(older).queryByText("Copied to clipboard")).not.toBeInTheDocument();
    expect(within(current).queryByRole("button", { name: "Copy" })).not.toBeInTheDocument();
    expect(within(current).getByText("Copied to clipboard")).toBeInTheDocument();
  });

  it("marks the preview belonging to the last active open viewer", () => {
    render(
      <ThumbnailCard
        artifact={artifact(null)}
        clipboardCurrent={false}
        viewerActive
        onRemoved={() => undefined}
      />,
    );

    expect(screen.getByRole("article")).toHaveClass("thumbnail-viewer-active");
  });

  it("starts the dismiss exit animation when the close control is clicked", () => {
    render(
      <ThumbnailCard
        artifact={artifact(null)}
        clipboardCurrent={false}
        viewerActive={false}
        onRemoved={() => undefined}
      />,
    );

    act(() => {
      screen.getByRole("button", { name: "Dismiss — available in History for 30 days" }).click();
    });
    expect(screen.getByRole("article")).toHaveClass("thumbnail-exit-dismiss");
  });

  it("starts the delete disintegration animation when Move to Trash is clicked", () => {
    render(
      <ThumbnailCard
        artifact={artifact("/Users/josevalerio/Captures/capture.png")}
        clipboardCurrent={false}
        viewerActive={false}
        onRemoved={() => undefined}
      />,
    );

    act(() => {
      screen.getByRole("button", { name: "Move to Trash" }).click();
    });
    const card = screen.getByRole("article");
    expect(card).toHaveClass("thumbnail-exit-delete");
    expect(card).toHaveClass("thumbnail-exit-dust");
    expect(card.querySelectorAll(".thumbnail-dust").length).toBeGreaterThan(100);
    expect(card.querySelector(".thumbnail-dust-layer")).not.toBeNull();
    expect(card.querySelector(".thumbnail-burn-front")).toBeNull();
  });

  it("freezes Saved! when exit starts instead of flipping to Show in Folder", async () => {
    vi.mocked(invoke).mockImplementation(async (command: string) => {
      if (command === "save_artifact") return undefined;
      return undefined;
    });

    const { rerender } = render(
      <ThumbnailCard
        artifact={artifact(null)}
        clipboardCurrent={false}
        viewerActive={false}
        onRemoved={() => undefined}
      />,
    );

    await act(async () => {
      screen.getByRole("button", { name: "Save" }).click();
      await Promise.resolve();
    });
    expect(screen.getByText("Saved!")).toBeInTheDocument();

    // Path appears after save while "Saved!" is still showing (same as real flow).
    rerender(
      <ThumbnailCard
        artifact={artifact("/Users/josevalerio/Captures/capture.png")}
        clipboardCurrent={false}
        viewerActive={false}
        onRemoved={() => undefined}
      />,
    );
    expect(screen.getByText("Saved!")).toBeInTheDocument();

    act(() => {
      screen.getByRole("button", { name: "Move to Trash" }).click();
    });
    const card = screen.getByRole("article");
    expect(card).toHaveClass("thumbnail-exit-delete");
    expect(card).toHaveClass("thumbnail-exiting");
    expect(card).toHaveAttribute("data-exit-locked", "true");
    expect(card).toHaveAttribute("aria-busy", "true");
    // Must stay frozen on Saved! — not swap to Show in Folder.
    expect(screen.getByText("Saved!")).toBeInTheDocument();
    expect(screen.queryByText("Show in Folder")).not.toBeInTheDocument();
    // Interactive controls are disabled for the whole exit animation.
    expect(screen.getByRole("button", { name: "Move to Trash" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "View Full Size" })).toBeDisabled();
  });
});
