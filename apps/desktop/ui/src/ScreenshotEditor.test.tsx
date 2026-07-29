import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

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
  });

  it("keeps PNG lossless by default and reveals JPEG quality only when selected", async () => {
    render(<ScreenshotEditor />);
    await screen.findAllByText("1440 × 900");

    const format = screen.getByLabelText("Format");
    expect(format).toHaveValue("png");
    expect(screen.queryByText("JPEG quality")).not.toBeInTheDocument();

    fireEvent.change(format, { target: { value: "jpeg" } });

    await waitFor(() => {
      expect(screen.getByText("JPEG quality")).toBeInTheDocument();
    });
    expect(screen.getByText("92%")).toBeInTheDocument();
  });
});
