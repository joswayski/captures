import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { act, createEvent, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

import { ScreenshotEditor } from "./ScreenshotEditor";
import type { CaptureArtifact } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
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

function installExportableCanvas(): () => void {
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

  const originalToBlob = Object.getOwnPropertyDescriptor(
    HTMLCanvasElement.prototype,
    "toBlob",
  );
  Object.defineProperty(HTMLCanvasElement.prototype, "toBlob", {
    configurable: true,
    value: (callback: BlobCallback, type?: string) => {
      const bytes = new Uint8Array(128);
      const blob = new Blob([bytes], { type: type ?? "image/png" });
      if (typeof blob.arrayBuffer !== "function") {
        Object.defineProperty(blob, "arrayBuffer", {
          value: async () => bytes.buffer,
        });
      }
      callback(blob);
    },
  });

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

  return () => {
    window.Image = originalImage;
    if (originalToBlob) {
      Object.defineProperty(HTMLCanvasElement.prototype, "toBlob", originalToBlob);
    } else {
      Reflect.deleteProperty(HTMLCanvasElement.prototype, "toBlob");
    }
  };
}

function setCanvasBounds(canvas: HTMLCanvasElement, width = 1_440, height = 900): void {
  vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({
    x: 0,
    y: 0,
    top: 0,
    left: 0,
    right: width,
    bottom: height,
    width,
    height,
    toJSON: () => ({}),
  });
}

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

    expect(await screen.findByLabelText("Width")).toHaveValue(1440);
    for (const name of [
      "Select & move (V)",
      "Crop (C)",
      "Text (T)",
      "Rectangle (R)",
      "Ellipse (O)",
      "Line (L)",
      "Arrow (A)",
      "Freehand (P)",
    ]) {
      expect(screen.getByRole("button", { name })).toBeInTheDocument();
    }
    expect(screen.queryByRole("button", { name: /Curved arrow/ })).not.toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("get_artifact", {
      artifactId: "capture-1",
    });
    const layers = screen.getByRole("region", { name: "Layers" });
    expect(within(layers).getByText("Original screenshot")).toBeInTheDocument();
    expect(within(layers).getByText("Locked background")).toBeInTheDocument();
    await waitFor(() => {
      expect(emit).toHaveBeenCalledWith("editor-layers-changed", {
        editor_id: "screenshot-editor-capture-1",
        artifact_ids: ["capture-1"],
      });
    });
  });

  it("clears editor presence when the original layer is deleted", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    const layers = screen.getByRole("region", { name: "Layers" });
    // Background starts locked; unlock before delete is allowed.
    fireEvent.click(within(layers).getByRole("button", {
      name: "Unlock Original screenshot",
    }));
    fireEvent.click(screen.getByRole("button", {
      name: /Original screenshotBackground/,
    }));
    fireEvent.click(screen.getByRole("button", { name: "Delete selected item" }));

    await waitFor(() => {
      expect(emit).toHaveBeenCalledWith("editor-layers-changed", {
        editor_id: "screenshot-editor-capture-1",
        artifact_ids: [],
      });
    });
  });

  it("zooms with the standard keyboard shortcuts", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    const zoom = screen.getByRole("combobox", { name: "Canvas zoom" });
    fireEvent.change(zoom, { target: { value: "100" } });
    expect(zoom).toHaveValue("100");

    const zoomIn = new KeyboardEvent("keydown", {
      key: "=",
      code: "Equal",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    });
    fireEvent(window, zoomIn);
    expect(zoomIn.defaultPrevented).toBe(true);
    expect(zoom).toHaveValue("125");
    expect(within(zoom).getByRole("option", { name: "125%" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "-", code: "Minus", ctrlKey: true });
    expect(zoom).toHaveValue("100");

    fireEvent.keyDown(window, { key: "+", code: "NumpadAdd", ctrlKey: true });
    expect(zoom).toHaveValue("125");
    fireEvent.keyDown(window, { key: "0", code: "Digit0", ctrlKey: true });
    expect(zoom).toHaveValue("100");
  });

  it("zooms on trackpad pinch or modified mouse wheel without consuming scroll", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    const viewport = screen.getByLabelText("Screenshot editing canvas");
    const zoom = screen.getByRole("combobox", { name: "Canvas zoom" });
    fireEvent.change(zoom, { target: { value: "100" } });

    const scroll = new WheelEvent("wheel", {
      deltaY: -100,
      bubbles: true,
      cancelable: true,
    });
    fireEvent(viewport, scroll);
    expect(scroll.defaultPrevented).toBe(false);
    expect(zoom).toHaveValue("100");

    const zoomIn = new WheelEvent("wheel", {
      deltaY: -100,
      ctrlKey: true,
      clientX: 320,
      clientY: 240,
      bubbles: true,
      cancelable: true,
    });
    fireEvent(viewport, zoomIn);
    expect(zoomIn.defaultPrevented).toBe(true);
    expect(Number((zoom as HTMLSelectElement).value))
      .toBeGreaterThan(100);

    const zoomOut = new WheelEvent("wheel", {
      deltaY: 100,
      metaKey: true,
      clientX: 320,
      clientY: 240,
      bubbles: true,
      cancelable: true,
    });
    fireEvent(viewport, zoomOut);
    expect(zoomOut.defaultPrevented).toBe(true);
    expect(Number((zoom as HTMLSelectElement).value)).toBeCloseTo(100, 1);
  });

  it("supports the native macOS magnify gesture", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    const viewport = screen.getByLabelText("Screenshot editing canvas");
    const zoom = screen.getByRole("combobox", { name: "Canvas zoom" });
    fireEvent.change(zoom, { target: { value: "100" } });

    const start = new Event("gesturestart", { bubbles: true, cancelable: true });
    Object.assign(start, { clientX: 300, clientY: 200, scale: 1 });
    fireEvent(viewport, start);
    expect(start.defaultPrevented).toBe(true);

    const change = new Event("gesturechange", { bubbles: true, cancelable: true });
    Object.assign(change, { clientX: 300, clientY: 200, scale: 1.5 });
    fireEvent(viewport, change);
    expect(change.defaultPrevented).toBe(true);
    expect(zoom).toHaveValue("150");

    const end = new Event("gestureend", { bubbles: true, cancelable: true });
    fireEvent(viewport, end);
    expect(end.defaultPrevented).toBe(true);
  });

  it("pans the viewport with Space-drag and Command/Ctrl-drag", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    const viewport = screen.getByLabelText("Screenshot editing canvas");
    const canvas = viewport.querySelector("canvas")!;
    viewport.setPointerCapture = vi.fn();
    viewport.hasPointerCapture = vi.fn(() => true);
    viewport.releasePointerCapture = vi.fn();
    viewport.scrollLeft = 200;
    viewport.scrollTop = 120;

    const spaceDown = new KeyboardEvent("keydown", {
      code: "Space",
      key: " ",
      bubbles: true,
      cancelable: true,
    });
    fireEvent(document.body, spaceDown);
    expect(spaceDown.defaultPrevented).toBe(true);
    expect(viewport).toHaveClass("is-pan-ready");

    fireEvent.pointerDown(canvas, {
      button: 0,
      buttons: 1,
      pointerId: 10,
      clientX: 400,
      clientY: 300,
    });
    expect(viewport).toHaveClass("is-panning");
    fireEvent.pointerMove(viewport, {
      buttons: 1,
      pointerId: 10,
      clientX: 350,
      clientY: 260,
    });
    expect(viewport.scrollLeft).toBe(250);
    expect(viewport.scrollTop).toBe(160);
    fireEvent.pointerUp(viewport, { button: 0, pointerId: 10 });
    expect(viewport).not.toHaveClass("is-panning");
    fireEvent.keyUp(document.body, { code: "Space", key: " " });
    expect(viewport).not.toHaveClass("is-pan-ready");

    viewport.scrollLeft = 90;
    viewport.scrollTop = 70;
    fireEvent.pointerDown(canvas, {
      button: 0,
      buttons: 1,
      pointerId: 11,
      clientX: 300,
      clientY: 220,
      metaKey: true,
    });
    fireEvent.pointerMove(viewport, {
      buttons: 1,
      pointerId: 11,
      clientX: 270,
      clientY: 200,
      metaKey: true,
    });
    expect(viewport.scrollLeft).toBe(120);
    expect(viewport.scrollTop).toBe(90);
    fireEvent.pointerUp(viewport, { button: 0, pointerId: 11, metaKey: true });
  });

  it("creates selectable formatted text directly on the canvas", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    fireEvent.change(screen.getByRole("combobox", { name: "Canvas zoom" }), {
      target: { value: "100" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Text (T)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    canvas.setPointerCapture = vi.fn();
    canvas.hasPointerCapture = vi.fn(() => true);
    canvas.releasePointerCapture = vi.fn();
    setCanvasBounds(canvas);
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 1,
      clientX: 120,
      clientY: 80,
    });

    const inlineEditor = await screen.findByRole("textbox", {
      name: "Edit text on canvas",
    });
    expect(inlineEditor).toHaveValue("Text");
    expect(inlineEditor).toHaveFocus();
    fireEvent.change(inlineEditor, { target: { value: "Inline text" } });
    expect(screen.getByRole("textbox", { name: "Text" })).toHaveValue("Inline text");
    expect(screen.getByRole("button", { name: "Bold" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Italic" })).toBeInTheDocument();
    expect(
      within(screen.getByRole("region", { name: "Layers" })).getByText("Inline text"),
    ).toBeInTheDocument();

    fireEvent.blur(inlineEditor);
    expect(screen.queryByRole("textbox", { name: "Edit text on canvas" }))
      .not.toBeInTheDocument();
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 2,
      clientX: 150,
      clientY: 100,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 2,
      clientX: 150,
      clientY: 100,
    });
    expect(await screen.findByRole("textbox", { name: "Edit text on canvas" }))
      .toHaveValue("Inline text");
  });

  it("copies, pastes, and duplicates the selected layer with standard shortcuts", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    fireEvent.click(screen.getByRole("button", { name: "Text (T)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    setCanvasBounds(canvas);
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 20,
      clientX: 120,
      clientY: 80,
    });
    fireEvent.blur(await screen.findByRole("textbox", { name: "Edit text on canvas" }));

    const layerList = screen.getByRole("region", { name: "Layers" })
      .querySelector(".screenshot-layer-list")!;
    expect(layerList.children).toHaveLength(2);

    const copy = new KeyboardEvent("keydown", {
      key: "c",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    });
    fireEvent(canvas, copy);
    expect(copy.defaultPrevented).toBe(true);
    const paste = new KeyboardEvent("keydown", {
      key: "v",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    });
    fireEvent(canvas, paste);
    expect(paste.defaultPrevented).toBe(true);
    expect(layerList.children).toHaveLength(3);

    const duplicate = new KeyboardEvent("keydown", {
      key: "d",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    fireEvent(canvas, duplicate);
    expect(duplicate.defaultPrevented).toBe(true);
    expect(layerList.children).toHaveLength(4);
  });

  it("draws one straight Arrow and bends it from its canvas control handle", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    fireEvent.change(screen.getByRole("combobox", { name: "Canvas zoom" }), {
      target: { value: "100" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Arrow (A)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    canvas.setPointerCapture = vi.fn();
    canvas.hasPointerCapture = vi.fn(() => true);
    canvas.releasePointerCapture = vi.fn();
    setCanvasBounds(canvas);
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 30,
      clientX: 100,
      clientY: 100,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 30,
      clientX: 300,
      clientY: 100,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 30,
      clientX: 300,
      clientY: 100,
    });

    const layers = screen.getByRole("region", { name: "Layers" });
    fireEvent.click(within(layers).getByRole("button", { name: /ArrowShape/ }));
    const curve = screen.getByRole("slider", { name: "Curve" });
    expect(curve).toHaveValue("0");

    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 31,
      clientX: 200,
      clientY: 100,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 31,
      clientX: 200,
      clientY: 200,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 31,
      clientX: 200,
      clientY: 200,
    });
    expect(curve).toHaveValue("50");
  });

  it("lets the original layer be unlocked and exposes layer appearance controls", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    const layers = screen.getByRole("region", { name: "Layers" });
    // One lock control on the layer row (status is the control's pressed state, not a second icon).
    const layerLock = within(layers).getByRole("button", {
      name: "Unlock Original screenshot",
    });
    expect(layerLock).toHaveAttribute("aria-pressed", "true");
    expect(layerLock).toHaveClass("active");
    expect(
      within(layers).queryByRole("button", { name: "Lock Original screenshot" }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Original screenshotLocked background/ }));
    expect(screen.getByRole("slider", { name: "Layer opacity" })).toHaveValue("100");
    expect(screen.getByLabelText("Blend mode")).toHaveValue("source-over");
    expect(screen.getByRole("button", { name: "Locked" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: "Visible" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    fireEvent.click(screen.getByRole("button", { name: "Locked" }));
    expect(screen.getByRole("button", { name: "Unlocked" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Duplicate" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Delete" })).toBeEnabled();
    expect(
      within(layers).getByRole("button", { name: "Lock Original screenshot" }),
    ).toHaveAttribute("aria-pressed", "false");
    expect(within(layers).getByText("Background")).toBeInTheDocument();
  });

  it("can clear the solid canvas background for transparent PNG/WebP exports", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    // Canvas background lives in the header toolbar (document chrome, not layers).
    const canvasToolbar = screen.getByRole("group", { name: "Canvas" });
    const solidBackground = within(canvasToolbar).getByRole("checkbox", {
      name: "Solid background",
    });
    expect(solidBackground).toBeChecked();
    // Compact ColorField keeps legend visually hidden but still exposes swatches.
    expect(within(canvasToolbar).getByLabelText("Canvas background: #ffffff")).toBeInTheDocument();

    const surface = screen
      .getByLabelText("Screenshot editing canvas")
      .querySelector(".screenshot-canvas-surface");
    expect(surface).not.toHaveClass("transparent");

    fireEvent.click(solidBackground);
    expect(solidBackground).not.toBeChecked();
    expect(within(canvasToolbar).queryByLabelText(/Canvas background:/)).not.toBeInTheDocument();
    expect(surface).toHaveClass("transparent");

    fireEvent.click(solidBackground);
    expect(solidBackground).toBeChecked();
    expect(within(canvasToolbar).getByLabelText("Canvas background: #ffffff")).toBeInTheDocument();
    expect(surface).not.toHaveClass("transparent");
  });

  it("offers Trim edges in the header canvas toolbar and disables it when already tight", async () => {
    render(<ScreenshotEditor />);
    const widthInput = await screen.findByLabelText("Width");
    const canvasToolbar = screen.getByRole("group", { name: "Canvas" });

    // Fresh capture fills the canvas — nothing to trim.
    const canvasTrim = within(canvasToolbar).getByRole("button", { name: "Trim edges" });
    expect(canvasTrim).toBeDisabled();

    // Manual canvas growth creates empty margin; trim should re-enable.
    fireEvent.change(widthInput, { target: { value: "1600" } });
    await waitFor(() => {
      expect(within(canvasToolbar).getByRole("button", { name: "Trim edges" })).toBeEnabled();
    });

    fireEvent.click(within(canvasToolbar).getByRole("button", { name: "Trim edges" }));
    await waitFor(() => {
      expect(screen.getByLabelText("Width")).toHaveValue(1440);
      expect(within(canvasToolbar).getByRole("button", { name: "Trim edges" })).toBeDisabled();
    });
  });

  it("does not select a new shape until Select & move is used", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

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

  it("deselects the active layer when clicking the empty viewport chrome", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    fireEvent.change(screen.getByRole("combobox", { name: "Canvas zoom" }), {
      target: { value: "100" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Text (T)" }));
    const viewport = screen.getByLabelText("Screenshot editing canvas");
    const canvas = viewport.querySelector("canvas")!;
    setCanvasBounds(canvas);
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 40,
      clientX: 120,
      clientY: 80,
    });
    fireEvent.blur(await screen.findByRole("textbox", { name: "Edit text on canvas" }));

    // Select the text layer (Select tool becomes active via layer list click).
    fireEvent.click(screen.getByRole("button", { name: "Select & move (V)" }));
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 41,
      clientX: 140,
      clientY: 95,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 41,
      clientX: 140,
      clientY: 95,
    });
    expect(screen.getByRole("button", { name: "Delete selected item" }))
      .toBeInTheDocument();

    // Click the checkerboard / empty padding around the canvas surface.
    fireEvent.pointerDown(viewport, {
      button: 0,
      pointerId: 42,
      clientX: -40,
      clientY: -30,
    });
    expect(screen.queryByRole("button", { name: "Delete selected item" }))
      .not.toBeInTheDocument();
  });

  it("can start an arrow outside the canvas and expands to fit on release", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    fireEvent.change(screen.getByRole("combobox", { name: "Canvas zoom" }), {
      target: { value: "100" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Arrow (A)" }));
    const viewport = screen.getByLabelText("Screenshot editing canvas");
    const canvas = viewport.querySelector("canvas")!;
    // Place the canvas away from the client origin so chrome clicks map to
    // negative document coordinates left/above the image.
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({
      x: 100,
      y: 80,
      top: 80,
      left: 100,
      right: 100 + 1_440,
      bottom: 80 + 900,
      width: 1_440,
      height: 900,
      toJSON: () => ({}),
    });
    viewport.setPointerCapture = vi.fn();
    viewport.hasPointerCapture = vi.fn(() => true);
    viewport.releasePointerCapture = vi.fn();

    // Start in the empty chrome left of the canvas, drag onto the image.
    fireEvent.pointerDown(viewport, {
      button: 0,
      pointerId: 50,
      clientX: 60,
      clientY: 200,
    });
    fireEvent.pointerMove(viewport, {
      pointerId: 50,
      clientX: 280,
      clientY: 260,
    });
    fireEvent.pointerUp(viewport, {
      button: 0,
      pointerId: 50,
      clientX: 280,
      clientY: 260,
    });

    expect(
      within(screen.getByRole("region", { name: "Layers" })).getByText("Arrow"),
    ).toBeInTheDocument();
    // Start was ~40px left of the canvas; stroke padding expands further.
    await waitFor(() => {
      const sizeLabels = screen.getAllByText(/\d+\s*×\s*\d+/);
      expect(sizeLabels.some((node) => {
        const match = node.textContent?.match(/(\d+)\s*×\s*(\d+)/);
        if (!match) return false;
        return Number(match[1]) > 1_440;
      })).toBe(true);
    });
  });

  it("snaps image drop guides to the closest edge without a selected layer", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    const editor = screen.getByText("Screenshot editor").closest("main");
    expect(editor).toBeTruthy();
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    // jsdom often reports 0×0 canvas layout; force a known client→document mapping.
    Object.defineProperty(canvas, "width", { configurable: true, value: 1_440 });
    Object.defineProperty(canvas, "height", { configurable: true, value: 900 });
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

    const dataTransfer = {
      types: ["Files"],
      dropEffect: "none",
      files: [],
    };

    fireEvent.dragEnter(editor!, { dataTransfer });
    // Default before a pointer sample is bottom.
    expect(screen.getAllByText("Place below").length).toBeGreaterThan(0);

    // Hover near the top edge without selecting a layer — previously always stayed "bottom".
    // jsdom drag events do not copy clientX/Y from fireEvent options; pin them on the event.
    const topOver = createEvent.dragOver(editor!, { dataTransfer });
    Object.defineProperty(topOver, "clientX", { configurable: true, value: 720 });
    Object.defineProperty(topOver, "clientY", { configurable: true, value: 40 });
    fireEvent(editor!, topOver);
    await waitFor(() => {
      expect(screen.getAllByText("Place above").length).toBeGreaterThan(0);
    });
    expect(screen.queryByText("Place below")).not.toBeInTheDocument();

    const guide = document.querySelector(".screenshot-drop-snap-guide.edge-top");
    expect(guide).not.toBeNull();
    expect(guide?.querySelector(".screenshot-drop-snap-bloom")).not.toBeNull();
    expect(guide?.querySelectorAll(".screenshot-drop-snap-particle").length).toBeGreaterThan(0);
  });

  it("offers stack-on-top placement with soft ambient under-glow (no discrete rays)", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    const editor = screen.getByText("Screenshot editor").closest("main");
    expect(editor).toBeTruthy();
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    // jsdom canvas layout is often 0×0; pin both backing store and client rect so
    // client→document mapping hits the layer interior (stack zone).
    Object.defineProperty(canvas, "width", { configurable: true, value: 1_440 });
    Object.defineProperty(canvas, "height", { configurable: true, value: 900 });
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

    const dataTransfer = {
      types: ["Files"],
      dropEffect: "none",
      files: [],
    };

    fireEvent.dragEnter(editor!, { dataTransfer });
    const stackOver = createEvent.dragOver(editor!, { dataTransfer });
    Object.defineProperty(stackOver, "clientX", { configurable: true, value: 720 });
    Object.defineProperty(stackOver, "clientY", { configurable: true, value: 450 });
    fireEvent(editor!, stackOver);
    await waitFor(() => {
      // Toast is the only "Place on top" label (no canvas badge for stack).
      expect(screen.getAllByText("Place on top")).toHaveLength(1);
    });
    expect(screen.getByText("Drop to place — stays editable as a layer.")).toBeInTheDocument();

    const guide = document.querySelector(".screenshot-drop-snap-guide.edge-stack") as HTMLElement | null;
    expect(guide).not.toBeNull();
    expect(guide?.querySelector(".screenshot-drop-snap-bloom")).not.toBeNull();
    // Soft ambient only — no extruded ray beams.
    expect(guide?.querySelector(".screenshot-drop-snap-stack-rays")).toBeNull();
    expect(guide?.querySelectorAll(".screenshot-drop-snap-stack-ray").length).toBe(0);
    // Stack mode does not render a second on-canvas label badge.
    expect(guide?.querySelector(":scope > span")).toBeNull();
    const plate = guide?.querySelector(".screenshot-drop-snap-stack-plate") as HTMLElement | null;
    expect(plate).not.toBeNull();
    expect(plate?.querySelector(".screenshot-drop-snap-stack-shadow")).not.toBeNull();
    expect(plate?.querySelector(".screenshot-drop-snap-stack-rim")).not.toBeNull();
    expect(plate?.querySelectorAll(".screenshot-drop-snap-particle").length).toBeGreaterThan(0);
    // Plate is a compact ghost relative to the target, centered near the pointer
    // (fit-mode displayScale can be tiny in jsdom, so compare ratios not CSS px).
    const guideWidth = Number.parseFloat(guide!.style.width);
    const guideHeight = Number.parseFloat(guide!.style.height);
    const plateLeft = Number.parseFloat(plate!.style.left);
    const plateTop = Number.parseFloat(plate!.style.top);
    const plateWidth = Number.parseFloat(plate!.style.width);
    const plateHeight = Number.parseFloat(plate!.style.height);
    expect(plateWidth).toBeGreaterThan(0);
    expect(plateHeight).toBeGreaterThan(0);
    expect(plateWidth / guideWidth).toBeLessThan(0.5);
    expect(plateHeight / guideHeight).toBeLessThan(0.5);
    expect((plateLeft + plateWidth / 2) / guideWidth).toBeCloseTo(720 / 1_440, 1);
    expect((plateTop + plateHeight / 2) / guideHeight).toBeCloseTo(450 / 900, 1);

    // Moving the pointer relocates the plate (dynamic, not a static center box).
    const moved = createEvent.dragOver(editor!, { dataTransfer });
    Object.defineProperty(moved, "clientX", { configurable: true, value: 900 });
    Object.defineProperty(moved, "clientY", { configurable: true, value: 520 });
    fireEvent(editor!, moved);
    await waitFor(() => {
      const nextPlate = document.querySelector(
        ".screenshot-drop-snap-guide.edge-stack .screenshot-drop-snap-stack-plate",
      ) as HTMLElement | null;
      expect(nextPlate).not.toBeNull();
      const nextLeft = Number.parseFloat(nextPlate!.style.left);
      const nextTop = Number.parseFloat(nextPlate!.style.top);
      expect(nextLeft).toBeGreaterThan(plateLeft);
      expect(nextTop).toBeGreaterThan(plateTop);
    });
  });

  it("keeps preserve quality by default and compress does not change the format", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    const format = screen.getByLabelText("Format");
    expect(format).toHaveValue("png");
    const saveQuality = screen.getByRole("combobox", { name: "Save quality" });
    expect(saveQuality).toHaveValue("preserve");
    expect(screen.queryByRole("slider", { name: "Image quality" })).not.toBeInTheDocument();
    expect(screen.queryByRole("spinbutton", { name: "Maximum file size" }))
      .not.toBeInTheDocument();
    expect(
      screen.getByText("Keeps original quality as PNG and replaces the original."),
    ).toBeInTheDocument();

    // Compress keeps PNG; the quality slider is JPEG-only.
    fireEvent.change(saveQuality, { target: { value: "compress" } });

    await waitFor(() => {
      expect(format).toHaveValue("png");
    });
    expect(screen.queryByRole("slider", { name: "Image quality" })).not.toBeInTheDocument();
    expect(screen.queryByRole("spinbutton", { name: "Maximum file size" }))
      .not.toBeInTheDocument();
    expect(
      screen.getByText("Compressed PNG replaces the original; turn on Make a copy to keep it."),
    ).toBeInTheDocument();

    fireEvent.change(format, { target: { value: "jpeg" } });
    expect(screen.getByRole("combobox", { name: "Save quality" })).toHaveValue("compress");
    expect(screen.getByRole("slider", { name: "Image quality" }))
      .toHaveAttribute("aria-valuetext", "Maximum");
    // Source is still a PNG path, so a JPEG save is always a new file.
    expect(
      screen.getByText("Compressed JPEG saves as a new file and leaves the original untouched."),
    ).toBeInTheDocument();

    fireEvent.change(screen.getByRole("combobox", { name: "Save quality" }), {
      target: { value: "maximum" },
    });

    expect(screen.queryByRole("slider", { name: "Image quality" })).not.toBeInTheDocument();
    expect(screen.getByRole("spinbutton", { name: "Maximum file size" })).toHaveValue(10);
    expect(screen.getByRole("combobox", { name: "Screenshot file size unit" }))
      .toHaveValue("mb");
    expect(format).toHaveValue("jpeg");

    // Switching format keeps the quality mode; maximum works for PNG too.
    fireEvent.change(format, { target: { value: "png" } });
    expect(screen.getByRole("combobox", { name: "Save quality" })).toHaveValue("maximum");
    expect(screen.getByRole("spinbutton", { name: "Maximum file size" })).toBeInTheDocument();
  });

  it("uses the original file size when export is original + preserve quality and unedited", async () => {
    const toBlob = vi.fn((callback: BlobCallback) => {
      callback(new Blob([new Uint8Array(999_999)], { type: "image/png" }));
    });
    Object.defineProperty(HTMLCanvasElement.prototype, "toBlob", {
      configurable: true,
      value: toBlob,
    });
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
      await screen.findByLabelText("Width");

      // Original PNG at full size with preserve quality → known capture size (250 KB).
      await waitFor(() => {
        expect(screen.getByText("≈ 250 KB")).toBeInTheDocument();
      }, { timeout: 2_000 });
      // Browser re-encode path should not run for the unedited original estimate.
      expect(toBlob).not.toHaveBeenCalled();
    } finally {
      window.Image = originalImage;
    }
  });

  it("supports explicit custom output width and height", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    fireEvent.change(screen.getByLabelText(/Output size/), {
      target: { value: "custom" },
    });
    const width = screen.getByRole("spinbutton", { name: "Custom output width" });
    const height = screen.getByRole("spinbutton", { name: "Custom output height" });
    expect(width).toHaveValue(1_440);
    expect(height).toHaveValue(900);

    fireEvent.change(width, { target: { value: "720" } });
    expect(height).toHaveValue(450);
    fireEvent.click(screen.getByRole("button", { name: "Lock output aspect ratio" }));
    fireEvent.change(height, { target: { value: "500" } });
    expect(width).toHaveValue(720);
    expect(height).toHaveValue(500);
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
      await screen.findByLabelText("Width");

      await waitFor(() => {
        expect(screen.getByText(/≈/)).toBeInTheDocument();
      }, { timeout: 2_000 });

      fireEvent.change(screen.getByRole("combobox", { name: "Save quality" }), {
        target: { value: "compress" },
      });
      // PNG compress keeps PNG; switch to JPEG to exercise the quality estimate path.
      expect(screen.getByLabelText("Format")).toHaveValue("png");
      fireEvent.change(screen.getByLabelText("Format"), { target: { value: "jpeg" } });
      await waitFor(() => {
        expect(screen.getByLabelText("Format")).toHaveValue("jpeg");
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

  it("names the destination before saving, honors the size mode, and reveals the result", async () => {
    const restoreCanvas = installExportableCanvas();
    const savedArtifact = {
      ...artifact,
      id: "capture-edited",
      path: "/Users/example/Pictures/edited-photo.png",
    };
    vi.mocked(open).mockResolvedValue("/Users/example/Pictures");
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifact") return artifact;
      if (command === "save_screenshot_edit") {
        return {
          artifact: savedArtifact,
          path: savedArtifact.path,
          format: "png",
        };
      }
      if (command === "reveal_artifact") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    try {
      render(<ScreenshotEditor />);
      expect(await screen.findByRole("textbox", { name: "Saved filename" }))
        .toHaveValue("capture");
      expect(screen.getByLabelText("Save location"))
        .toHaveTextContent("/Users/example/Captures");
      expect(screen.getByRole("checkbox", { name: "Make a copy" })).not.toBeChecked();

      fireEvent.click(screen.getByRole("button", { name: "Change save location" }));
      await waitFor(() => {
        expect(screen.getByLabelText("Save location"))
          .toHaveTextContent("/Users/example/Pictures");
      });
      expect(open).toHaveBeenCalledWith({
        directory: true,
        multiple: false,
        title: "Choose save location",
        defaultPath: "/Users/example/Captures",
      });

      fireEvent.change(screen.getByRole("textbox", { name: "Saved filename" }), {
        target: { value: "edited-photo" },
      });
      fireEvent.change(screen.getByRole("combobox", { name: "Save quality" }), {
        target: { value: "maximum" },
      });
      // Maximum size keeps the selected PNG format.
      expect(screen.getByLabelText("Format")).toHaveValue("png");
      await act(async () => undefined);
      fireEvent.click(screen.getByRole("button", { name: "Save" }));

      await waitFor(() => {
        expect(screen.queryByRole("alert")).not.toBeInTheDocument();
        expect(invoke).toHaveBeenCalledWith(
          "save_screenshot_edit",
          {
            request: expect.objectContaining({
              artifact_id: artifact.id,
              destination_path: "/Users/example/Pictures/edited-photo.png",
              format: "png",
              quality_mode: "maximum",
              jpeg_quality: 100,
              max_size_bytes: 10_000_000,
              overwrite_source: false,
            }),
          },
        );
      });
      expect(invoke).toHaveBeenCalledWith("reveal_artifact", {
        artifactId: savedArtifact.id,
      });
      // After the first copy save, the editor treats that file as the original
      // so later Save overwrites and Make a copy creates another file.
      await waitFor(() => {
        expect(screen.getByRole("textbox", { name: "Saved filename" }))
          .toHaveValue("edited-photo");
        expect(screen.getByLabelText("Save location"))
          .toHaveTextContent("/Users/example/Pictures");
        expect(screen.getByRole("checkbox", { name: "Make a copy" })).not.toBeChecked();
      });
      fireEvent.click(screen.getByRole("checkbox", { name: "Make a copy" }));
      expect(screen.getByRole("textbox", { name: "Saved filename" }))
        .toHaveValue("edited-photo-edited");
    } finally {
      restoreCanvas();
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
    await screen.findByLabelText("Width");

    expect(screen.getByRole("button", { name: "Copy image" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    expect(screen.getByRole("checkbox", { name: "Make a copy" })).not.toBeChecked();
    expect(
      screen.getByText("Keeps original quality as PNG and replaces the original."),
    ).toBeInTheDocument();

    expect(artifactRemoved).not.toBeNull();
    act(() => {
      artifactRemoved!({ payload: "capture-1" });
    });

    expect(
      screen.getByText(
        "The original was deleted. You can still copy or save this edit.",
      ),
    ).toHaveClass("screenshot-export-hint");
    expect(
      screen.queryByText("Keeps original quality as PNG and replaces the original."),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy image" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    expect(screen.queryByRole("checkbox", { name: "Make a copy" }))
      .not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows a success state for copy without replacing the export hint", async () => {
    const restoreCanvas = installExportableCanvas();
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifact") return artifact;
      if (command === "copy_screenshot_edit") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    try {
      render(<ScreenshotEditor />);
      await screen.findByLabelText("Width");

      const hint = "Keeps original quality as PNG and replaces the original.";
      expect(screen.getByText(hint)).toBeInTheDocument();

      fireEvent.click(screen.getByRole("button", { name: "Copy image" }));

      await waitFor(() => {
        expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
      });
      expect(screen.getByText("Copied to clipboard")).toHaveClass("success");
      // Hint stays put so the footer does not reflow around a status swap.
      expect(screen.getByText(hint)).toBeInTheDocument();
      expect(invoke).toHaveBeenCalledWith("copy_screenshot_edit", {
        imagePng: expect.any(Array),
      });
    } finally {
      restoreCanvas();
    }
  });
});
