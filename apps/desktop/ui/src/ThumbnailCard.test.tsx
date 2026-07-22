import { invoke } from "@tauri-apps/api/core";
import { act, fireEvent, render, screen, within } from "@testing-library/react";

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
  it("renders the quick preview from the full-resolution image and highlights it once ready", async () => {
    render(<ThumbnailCard artifact={artifact(null)} clipboardCurrent viewerActive={false} onRemoved={() => undefined} />);

    const image = screen.getByRole("img", { name: "Screenshot preview" });
    expect(image)
      .toHaveAttribute("src", "captures-capture://artifact-full/capture-1");
    expect(screen.getByRole("article")).not.toHaveClass("thumbnail-capture-highlight");

    await act(async () => {
      fireEvent.load(image);
      await Promise.resolve();
    });

    expect(screen.getByRole("article")).toHaveClass("thumbnail-capture-highlight");
  });

  it("latches the hover presentation before the full-size viewer takes focus", () => {
    render(<ThumbnailCard artifact={artifact(null)} clipboardCurrent viewerActive={false} onRemoved={() => undefined} />);

    const card = screen.getByRole("article");
    act(() => {
      screen.getByRole("button", { name: "Full size" }).click();
    });

    expect(card).toHaveClass("thumbnail-card-native-active");
    expect(invoke).toHaveBeenCalledWith("open_artifact_viewer", { artifactId: "capture-1" });
  });

  it("before a folder save: Delete only (no Close), Save file for a disk PNG", () => {
    render(<ThumbnailCard artifact={artifact(null)} clipboardCurrent viewerActive={false} onRemoved={() => undefined} />);

    expect(screen.getByText("Copied to clipboard")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Full size" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Close" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Copy" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save file" })).toBeInTheDocument();
  });

  it("after a folder save: Close keeps the file, Delete removes it", () => {
    render(
      <ThumbnailCard
        artifact={artifact("/Users/josevalerio/Captures/capture.png")}
        clipboardCurrent
        viewerActive={false}
        onRemoved={() => undefined}
      />,
    );

    const close = screen.getByRole("button", { name: "Close" });
    const trash = screen.getByRole("button", { name: "Delete" });
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

    expect(screen.getByRole("button", { name: "Delete" })).toBeInTheDocument();
    expect(screen.getByText("Not in History")).toBeInTheDocument();
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

  it("deletes an unsaved preview with the dissolve animation", () => {
    render(
      <ThumbnailCard
        artifact={artifact(null)}
        clipboardCurrent={false}
        viewerActive={false}
        onRemoved={() => undefined}
      />,
    );

    act(() => {
      screen.getByRole("button", { name: "Delete" }).click();
    });
    const card = screen.getByRole("article");
    expect(card).toHaveClass("thumbnail-exit-delete");
    expect(card).toHaveClass("thumbnail-exit-dust");
  });

  it("starts the delete disintegration animation when Delete is clicked after save", () => {
    render(
      <ThumbnailCard
        artifact={artifact("/Users/josevalerio/Captures/capture.png")}
        clipboardCurrent={false}
        viewerActive={false}
        onRemoved={() => undefined}
      />,
    );

    act(() => {
      screen.getByRole("button", { name: "Delete" }).click();
    });
    const card = screen.getByRole("article");
    expect(card).toHaveClass("thumbnail-exit-delete");
    expect(card).toHaveClass("thumbnail-exit-dust");
    expect(card.querySelectorAll(".thumbnail-dust").length).toBeGreaterThan(100);
    expect(card.querySelector(".thumbnail-dust-layer")).not.toBeNull();
  });

  it("freezes Saved when exit starts instead of flipping to Show in Folder", async () => {
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
      screen.getByRole("button", { name: "Save file" }).click();
      await Promise.resolve();
    });
    expect(screen.getByText("Saved")).toBeInTheDocument();

    rerender(
      <ThumbnailCard
        artifact={artifact("/Users/josevalerio/Captures/capture.png")}
        clipboardCurrent={false}
        viewerActive={false}
        onRemoved={() => undefined}
      />,
    );
    expect(screen.getByText("Saved")).toBeInTheDocument();

    act(() => {
      screen.getByRole("button", { name: "Delete" }).click();
    });
    const card = screen.getByRole("article");
    expect(card).toHaveClass("thumbnail-exit-delete");
    expect(card).toHaveClass("thumbnail-exiting");
    expect(card).toHaveAttribute("data-exit-locked", "true");
    expect(screen.getByText("Saved")).toBeInTheDocument();
    expect(screen.queryByText("Show in Folder")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Full size" })).toBeDisabled();
  });
});
