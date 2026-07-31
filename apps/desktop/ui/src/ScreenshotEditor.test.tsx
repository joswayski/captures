import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

import { ScreenshotEditor } from "./ScreenshotEditor";
import type { CaptureArtifact } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: vi.fn(async () => null),
}));

const artifact: CaptureArtifact = {
  id: "capture-1",
  path: "/Users/example/Captures/capture.png",
  preview_url: "captures-capture://localhost/artifact/capture-1",
  full_url: "captures-capture://localhost/artifact-full/capture-1",
  width: 1_440,
  height: 900,
  size_bytes: 250_000,
  created_at: "2026-07-29T00:00:00Z",
  mode: "region",
  history_saved: true,
  clipboard_copy_status: "copied",
};

describe("ScreenshotEditor", () => {
  beforeEach(() => {
    window.history.replaceState({}, "", "/?view=screenshot-editor&artifact_id=capture-1");
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifact") return artifact;
      throw new Error(`unexpected command: ${command}`);
    });
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("loads the full-resolution artifact and exposes every requested annotation tool", async () => {
    render(<ScreenshotEditor />);

    expect((await screen.findAllByText("1440 × 900")).length).toBeGreaterThan(0);
    for (const name of [
      "Select & move (V)",
      "Crop (C)",
      "Text (T)",
      "Rectangle (R)",
      "Ellipse (O)",
      "Line (L)",
      "Arrow (A)",
      "Curved arrow (B)",
      "Freehand (P)",
    ]) {
      expect(screen.getByRole("button", { name })).toBeInTheDocument();
    }
    expect(invoke).toHaveBeenCalledWith("get_artifact", {
      artifactId: "capture-1",
    });
    const layers = screen.getByRole("region", { name: "Layers" });
    expect(within(layers).getByText("Original screenshot")).toBeInTheDocument();
    expect(within(layers).getByText("Locked background")).toBeInTheDocument();
  });

  it("creates selectable formatted text directly on the canvas", async () => {
    render(<ScreenshotEditor />);
    await screen.findAllByText("1440 × 900");

    fireEvent.click(screen.getByRole("button", { name: "Text (T)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1_440,
      bottom: 900,
      width: 1_440,
      height: 900,
      toJSON: () => ({}),
    });
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 1,
      clientX: 120,
      clientY: 80,
    });

    expect(await screen.findByRole("textbox", { name: "Text" })).toHaveValue("Text");
    expect(screen.getByRole("button", { name: "Bold" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Italic" })).toBeInTheDocument();
    expect(
      within(screen.getByRole("region", { name: "Layers" })).getAllByText("Text"),
    ).toHaveLength(2);
  });

  it("does not select a new shape until Select & move is used", async () => {
    render(<ScreenshotEditor />);
    await screen.findAllByText("1440 × 900");

    fireEvent.click(screen.getByRole("button", { name: "Rectangle (R)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    canvas.setPointerCapture = vi.fn();
    canvas.hasPointerCapture = vi.fn(() => true);
    canvas.releasePointerCapture = vi.fn();
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 1_440,
      bottom: 900,
      width: 1_440,
      height: 900,
      toJSON: () => ({}),
    });
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 2,
      clientX: 100,
      clientY: 100,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 2,
      clientX: 300,
      clientY: 240,
    });
    fireEvent.pointerUp(canvas, {
      pointerId: 2,
      clientX: 300,
      clientY: 240,
    });

    expect(screen.queryByRole("button", { name: "Delete selected item" }))
      .not.toBeInTheDocument();
    expect(
      within(screen.getByRole("region", { name: "Layers" })).getByText("Rectangle"),
    ).toBeInTheDocument();
  });

  it("uses maximum lossless output by default and a notched JPEG quality control", async () => {
    render(<ScreenshotEditor />);
    await screen.findAllByText("1440 × 900");

    const format = screen.getByLabelText("Format");
    expect(format).toHaveValue("png");
    expect(screen.getByText("Maximum · lossless")).toBeInTheDocument();
    expect(screen.queryByRole("slider", { name: "Image quality" })).not.toBeInTheDocument();
    expect(screen.getByRole("spinbutton", { name: "Maximum file size" })).toHaveValue(null);

    fireEvent.change(format, { target: { value: "jpeg" } });

    await waitFor(() => {
      expect(screen.getByRole("slider", { name: "Image quality" })).toBeInTheDocument();
    });
    expect(screen.getByRole("slider", { name: "Image quality" }))
      .toHaveAttribute("aria-valuetext", "Maximum");
  });

  it("shows an estimated export size when format and quality change", async () => {
    const toBlob = vi.fn((
      callback: BlobCallback,
      type?: string,
      quality?: number,
    ) => {
      const size = type === "image/jpeg"
        ? Math.max(1, Math.round(80_000 * (quality ?? 1)))
        : 220_000;
      callback(new Blob([new Uint8Array(size)], { type: type ?? "image/png" }));
    });
    const context = {
      canvas: document.createElement("canvas"),
      drawImage: vi.fn(),
      fillRect: vi.fn(),
      clearRect: vi.fn(),
      save: vi.fn(),
      restore: vi.fn(),
      beginPath: vi.fn(),
      moveTo: vi.fn(),
      lineTo: vi.fn(),
      stroke: vi.fn(),
      fill: vi.fn(),
      arc: vi.fn(),
      closePath: vi.fn(),
      setLineDash: vi.fn(),
      measureText: () => ({ width: 40 }),
      fillText: vi.fn(),
      strokeText: vi.fn(),
      translate: vi.fn(),
      scale: vi.fn(),
      rotate: vi.fn(),
      quadraticCurveTo: vi.fn(),
      imageSmoothingEnabled: true,
      imageSmoothingQuality: "high",
      createLinearGradient: () => ({ addColorStop: vi.fn() }),
    } as unknown as CanvasRenderingContext2D;
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(context);
    Object.defineProperty(HTMLCanvasElement.prototype, "toBlob", {
      configurable: true,
      value: toBlob,
    });

    // Background screenshot image must report as loaded for export estimation.
    const originalImage = window.Image;
    class LoadedImage {
      onload: ((event: Event) => void) | null = null;
      onerror: ((event: Event) => void) | null = null;
      naturalWidth = 1_440;
      naturalHeight = 900;
      width = 1_440;
      height = 900;
      crossOrigin = "";
      set src(_value: string) {
        queueMicrotask(() => this.onload?.(new Event("load")));
      }
    }
    // @ts-expect-error test stub for Image load timing
    window.Image = LoadedImage;

    try {
      render(<ScreenshotEditor />);
      await screen.findAllByText("1440 × 900");

      await waitFor(() => {
        expect(screen.getByText(/≈/)).toBeInTheDocument();
      }, { timeout: 2_000 });

      fireEvent.change(screen.getByLabelText("Format"), { target: { value: "jpeg" } });
      await waitFor(() => {
        expect(screen.getByRole("slider", { name: "Image quality" })).toBeInTheDocument();
      });
      fireEvent.change(screen.getByRole("slider", { name: "Image quality" }), {
        target: { value: "0" },
      });

      await waitFor(() => {
        expect(toBlob.mock.calls.some((call) => call[1] === "image/jpeg")).toBe(true);
        expect(screen.getByText(/≈/)).toBeInTheDocument();
      }, { timeout: 2_000 });
    } finally {
      window.Image = originalImage;
    }
  });

  it("keeps copy and save available when the original capture is deleted", async () => {
    type ArtifactRemovedHandler = (event: { payload: string }) => void;
    let artifactRemoved: ArtifactRemovedHandler | null = null;
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "artifact-removed") {
        artifactRemoved = handler as ArtifactRemovedHandler;
      }
      return () => undefined;
    });

    render(<ScreenshotEditor />);
    await screen.findAllByText("1440 × 900");

    expect(screen.getByRole("button", { name: "Copy" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Save copy" })).toBeEnabled();
    expect(
      screen.getByText("Saving creates a new copy and preserves the original."),
    ).toBeInTheDocument();

    expect(artifactRemoved).not.toBeNull();
    act(() => {
      artifactRemoved!({ payload: "capture-1" });
    });

    expect(
      screen.getByText(
        "The original was deleted. You can still copy or save this edit.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Save copy" })).toBeEnabled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
