import { invoke, isTauri } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { act, createEvent, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

import {
  createScreenshotDocument,
  elementBounds,
  resizeHandlePoint,
  type EditorShapeElement,
} from "./lib/screenshotEditor";
import { ScreenshotEditor } from "./ScreenshotEditor";
import type { CaptureArtifact } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: vi.fn(() => false),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    onCloseRequested: vi.fn(async () => () => undefined),
    destroy: vi.fn(async () => undefined),
  })),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
}));

/** Shared no-op handlers for draft autosave commands used by every editor test. */
function draftCommandResult(command: string): unknown {
  if (command === "load_screenshot_editor_draft") return null;
  if (command === "save_screenshot_editor_draft") return undefined;
  if (command === "discard_screenshot_editor_draft") return undefined;
  if (command === "get_settings") return { screenshot_format: "png" };
  return undefined;
}

/** Match ScreenshotEditor log-scale slider mapping (5%–800%). */
const TEST_ZOOM_MIN = 5;
const TEST_ZOOM_MAX = 800;
const TEST_ZOOM_LOG_SPAN = Math.log(TEST_ZOOM_MAX / TEST_ZOOM_MIN);

function zoomPercentToSliderPosition(percent: number): number {
  const clamped = Math.min(TEST_ZOOM_MAX, Math.max(TEST_ZOOM_MIN, percent));
  return Math.log(clamped / TEST_ZOOM_MIN) / TEST_ZOOM_LOG_SPAN;
}

function setCanvasZoomPercent(percent: number) {
  const zoom = screen.getByRole("slider", { name: "Canvas zoom" });
  fireEvent.change(zoom, {
    target: { value: String(zoomPercentToSliderPosition(percent)) },
  });
  return zoom;
}

function canvasZoomPercent(zoom: HTMLElement = screen.getByRole("slider", { name: "Canvas zoom" })): number {
  const text = zoom.getAttribute("aria-valuetext") ?? "";
  const match = text.match(/([\d.]+)%/);
  if (!match) {
    throw new Error(`Expected zoom aria-valuetext with a percent, got "${text}"`);
  }
  return Number(match[1]);
}

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
    measureText: (text: string) => ({
      width: Math.max(1, [...String(text ?? "")].length * 10),
    }),
    fillText: vi.fn(),
    strokeText: vi.fn(),
    translate: vi.fn(),
    transform: vi.fn(),
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
    vi.mocked(isTauri).mockReturnValue(false);
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_artifact") return artifact;
      if (command === "estimate_screenshot_export") {
        const colors = Number((args as { pngMaxColors?: number } | undefined)?.pngMaxColors ?? 128);
        return Math.max(8_000, Math.round(1_200 * colors));
      }
      if (command === "preview_screenshot_export") {
        const colors = Number((args as { pngMaxColors?: number } | undefined)?.pngMaxColors ?? 128);
        const size = Math.max(8_000, Math.round(1_200 * colors));
        return {
          bytes: Array.from({ length: Math.min(size, 256) }, (_, index) => index % 256),
          sizeBytes: size,
          format: (args as { format?: string } | undefined)?.format ?? "png",
        };
      }
      const draft = draftCommandResult(String(command));
      if (draft !== undefined || String(command).includes("screenshot_editor_draft")) {
        return draft;
      }
      throw new Error(`unexpected command: ${command}`);
    });
    Object.assign(URL, {
      createObjectURL: vi.fn(() => "blob:compress-preview"),
      revokeObjectURL: vi.fn(),
    });
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("restores a saved editor draft and can discard it", async () => {
    const draftDocument = createScreenshotDocument(
      "captures-capture://localhost/editor-draft/capture-1/asset-1",
      1_200,
      800,
      "capture-1",
    );
    draftDocument.background = null;
    draftDocument.elements.push({
      id: "note-1",
      kind: "text",
      text: "Draft note",
      fontSize: 32,
      width: 200,
      fontFamily: "sans",
      bold: false,
      italic: false,
      align: "left",
      color: "#fff",
      background: null,
      outlined: false,
      roundedBackground: false,
      x: 20,
      y: 30,
      locked: false,
      visible: true,
      opacity: 100,
      blendMode: "source-over",
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifact") return artifact;
      if (command === "load_screenshot_editor_draft") {
        return { document: draftDocument, updated_at_ms: 1 };
      }
      if (command === "save_screenshot_editor_draft") return undefined;
      if (command === "discard_screenshot_editor_draft") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<ScreenshotEditor />);

    expect(await screen.findByText("Restored unsaved edits from last time.")).toBeInTheDocument();
    expect(screen.getByLabelText("Canvas width")).toHaveValue(1_200);
    expect(screen.getByText("Draft note")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Discard" }));

    await waitFor(() => {
      expect(screen.queryByText("Restored unsaved edits from last time.")).not.toBeInTheDocument();
    });
    expect(screen.getByLabelText("Canvas width")).toHaveValue(1_440);
    expect(screen.queryByText("Draft note")).not.toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("discard_screenshot_editor_draft", {
      artifactId: "capture-1",
    });
  });

  it("flushes the editor draft on close without preventDefault so the window can close", async () => {
    // Regression: PR #187 always preventDefault()'d then called destroy() manually.
    // Tauri only destroys after onCloseRequested when preventDefault was NOT called,
    // so the first X click cleared mini-preview presence but left the editor open.
    type CloseHandler = (event: { preventDefault: () => void }) => void | Promise<void>;
    let closeHandler: CloseHandler | null = null;
    const destroy = vi.fn(async () => undefined);
    const onCloseRequested = vi.fn(async (handler: CloseHandler) => {
      closeHandler = handler;
      return () => undefined;
    });
    vi.mocked(isTauri).mockReturnValue(true);
    vi.mocked(getCurrentWindow).mockReturnValue({
      onCloseRequested,
      destroy,
    } as unknown as ReturnType<typeof getCurrentWindow>);

    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    await waitFor(() => {
      expect(closeHandler).not.toBeNull();
    });

    const preventDefault = vi.fn();
    await act(async () => {
      await closeHandler!({ preventDefault });
    });

    expect(preventDefault).not.toHaveBeenCalled();
    // Let Tauri destroy after the handler; do not force-close from the app.
    expect(destroy).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("discard_screenshot_editor_draft", {
        artifactId: "capture-1",
      });
    });
  });

  it("resolves the close handler even when draft flush never finishes", async () => {
    // Regression: awaiting an unbounded draft encode/IPC made the red X a no-op
    // because Tauri only destroy()s after onCloseRequested settles.
    type CloseHandler = (event: { preventDefault: () => void }) => void | Promise<void>;
    let closeHandler: CloseHandler | null = null;
    const onCloseRequested = vi.fn(async (handler: CloseHandler) => {
      closeHandler = handler;
      return () => undefined;
    });
    vi.mocked(isTauri).mockReturnValue(true);
    vi.mocked(getCurrentWindow).mockReturnValue({
      onCloseRequested,
      destroy: vi.fn(async () => undefined),
    } as unknown as ReturnType<typeof getCurrentWindow>);

    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "discard_screenshot_editor_draft") {
        return new Promise(() => {
          /* never resolves — simulates a stuck draft write */
        });
      }
      if (command === "get_artifact") return artifact;
      if (command === "estimate_screenshot_export") {
        const quality = Number((args as { jpegQuality?: number } | undefined)?.jpegQuality ?? 92);
        return Math.max(8_000, Math.round(120_000 * (quality / 100)));
      }
      const draft = draftCommandResult(String(command));
      if (draft !== undefined || String(command).includes("screenshot_editor_draft")) {
        return draft;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");
    await waitFor(() => {
      expect(closeHandler).not.toBeNull();
    });

    const preventDefault = vi.fn();
    const started = performance.now();
    await act(async () => {
      await closeHandler!({ preventDefault });
    });
    const elapsedMs = performance.now() - started;

    expect(preventDefault).not.toHaveBeenCalled();
    // Must settle via the close-flush timeout (~400ms), not hang indefinitely.
    expect(elapsedMs).toBeLessThan(2_000);
  });

  it("loads the full-resolution artifact and exposes every requested annotation tool", async () => {
    render(<ScreenshotEditor />);

    expect(await screen.findByLabelText("Canvas width")).toHaveValue(1440);
    for (const name of [
      "Select & move (V)",
      "Crop (C)",
      "Text (T)",
      "Rectangle (R)",
      "Ellipse (O)",
      "Line (L)",
      "Arrow (A)",
      "Freehand (P)",
      "Eraser (B)",
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

  it("keeps locked layer transforms disabled and lets unlocked height scale proportionally", async () => {
    render(<ScreenshotEditor />);
    const canvasWidth = await screen.findByLabelText("Canvas width");
    const canvasHeight = screen.getByLabelText("Canvas height");
    expect(canvasWidth).toBeEnabled();
    expect(canvasHeight).toBeEnabled();
    expect(screen.getByRole("group", { name: "Canvas" })).toHaveTextContent("Canvas");

    fireEvent.click(screen.getByRole("button", {
      name: /Original screenshotLocked background/,
    }));

    const layerWidth = screen.getByLabelText("Layer width");
    const layerHeight = screen.getByLabelText("Layer height");
    const layerX = screen.getByLabelText("Layer X");
    const layerY = screen.getByLabelText("Layer Y");

    expect(layerWidth).toBeDisabled();
    expect(layerHeight).toBeDisabled();
    expect(layerX).toBeDisabled();
    expect(layerY).toBeDisabled();
    expect(layerWidth).toHaveValue(1440);
    expect(layerHeight).toHaveValue(900);
    expect(screen.queryByRole("button", { name: "Increase Layer width" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Increase Layer height" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Increase Canvas width" })).toBeEnabled();
    expect(screen.getByText("Unlock this layer to change size and position.")).toBeInTheDocument();

    fireEvent.change(canvasWidth, { target: { value: "1200" } });
    expect(canvasWidth).toHaveValue(1200);
    expect(layerWidth).toHaveValue(1440);

    fireEvent.click(screen.getByRole("button", { name: "Unlock Original screenshot" }));

    expect(layerWidth).toBeEnabled();
    expect(layerHeight).toBeEnabled();
    expect(layerX).toBeEnabled();
    expect(layerY).toBeEnabled();
    expect(screen.getByRole("button", { name: "Increase Layer height" })).toBeEnabled();
    expect(screen.getByText("Width and height stay proportional to the image.")).toBeInTheDocument();

    fireEvent.change(layerHeight, { target: { value: "450" } });
    expect(layerHeight).toHaveValue(450);
    expect(layerWidth).toHaveValue(720);
    expect(canvasWidth).toHaveValue(1200);
    expect(canvasHeight).toHaveValue(900);
  });

  it("keeps advanced export controls behind a compact disclosure", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const disclosure = screen.getByRole("button", { name: /Export settings/ });
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    const filename = screen.getByRole("textbox", { name: "Saved filename" });
    const format = screen.getByRole("combobox", { name: "Format" });
    expect(format).toHaveTextContent(".png");
    expect(filename.closest(".recording-filename-input")).toContainElement(format);

    fireEvent.click(disclosure);

    expect(disclosure).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("combobox", { name: "Save quality" })).toBeInTheDocument();
  });

  it("changes screenshot format from the filename extension without opening export settings", async () => {
    render(<ScreenshotEditor />);
    await screen.findByRole("textbox", { name: "Saved filename" });

    const disclosure = screen.getByRole("button", { name: /Export settings/ });
    const format = screen.getByRole("combobox", { name: "Format" });
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    expect(format).toHaveTextContent(".png");
    expect(disclosure).toHaveTextContent(/PNG/);

    fireEvent.click(format);
    fireEvent.click(screen.getByRole("option", { name: "JPEG" }));
    expect(format).toHaveTextContent(".jpg");
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    expect(disclosure).toHaveTextContent(/JPEG/);

    fireEvent.click(format);
    fireEvent.click(screen.getByRole("option", { name: "WebP" }));
    expect(format).toHaveTextContent(".webp");
    expect(disclosure).toHaveTextContent(/WebP/);
  });

  it("clears editor presence when the original layer is deleted", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const layers = screen.getByRole("region", { name: "Layers" });
    // Background starts locked; unlock before delete is allowed.
    fireEvent.click(within(layers).getByRole("button", {
      name: "Unlock Original screenshot",
    }));
    fireEvent.click(screen.getByRole("button", {
      name: /Original screenshotBackground/,
    }));
    fireEvent.click(within(layers).getByRole("button", {
      name: "Layer settings for Original screenshot",
    }));
    fireEvent.click(within(screen.getByRole("dialog", {
      name: "Layer settings for Original screenshot",
    })).getByRole("button", { name: /Delete/ }));

    await waitFor(() => {
      expect(emit).toHaveBeenCalledWith("editor-layers-changed", {
        editor_id: "screenshot-editor-capture-1",
        artifact_ids: [],
      });
    });
  });

  it("zooms with the standard keyboard shortcuts", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const zoom = setCanvasZoomPercent(100);
    expect(canvasZoomPercent(zoom)).toBeCloseTo(100, 0);

    const zoomIn = new KeyboardEvent("keydown", {
      key: "=",
      code: "Equal",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    });
    fireEvent(window, zoomIn);
    expect(zoomIn.defaultPrevented).toBe(true);
    expect(canvasZoomPercent(zoom)).toBeCloseTo(125, 0);
    const preset = screen.getByRole("combobox", { name: "Canvas zoom preset" });
    expect(within(preset).getByRole("option", { name: "125%" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "-", code: "Minus", ctrlKey: true });
    expect(canvasZoomPercent(zoom)).toBeCloseTo(100, 0);

    fireEvent.keyDown(window, { key: "+", code: "NumpadAdd", ctrlKey: true });
    expect(canvasZoomPercent(zoom)).toBeCloseTo(125, 0);
    fireEvent.keyDown(window, { key: "0", code: "Digit0", ctrlKey: true });
    expect(canvasZoomPercent(zoom)).toBeCloseTo(100, 0);
  });

  it("zooms with the header slider and zoom buttons", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const zoom = setCanvasZoomPercent(100);
    expect(canvasZoomPercent(zoom)).toBeCloseTo(100, 0);
    // ~100% sits mid-track on the log scale (not near the left end).
    expect(Number((zoom as HTMLInputElement).value)).toBeGreaterThan(0.45);
    expect(Number((zoom as HTMLInputElement).value)).toBeLessThan(0.7);

    setCanvasZoomPercent(175);
    expect(canvasZoomPercent(zoom)).toBeCloseTo(175, 0);

    fireEvent.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(canvasZoomPercent(zoom)).toBeCloseTo(218.8, 0);

    fireEvent.click(screen.getByRole("button", { name: "Zoom out" }));
    expect(canvasZoomPercent(zoom)).toBeCloseTo(175, 0);

    fireEvent.change(screen.getByRole("combobox", { name: "Canvas zoom preset" }), {
      target: { value: "50" },
    });
    expect(canvasZoomPercent(zoom)).toBeCloseTo(50, 0);

    fireEvent.click(screen.getByRole("button", { name: "Fit canvas" }));
    expect(screen.getByRole("combobox", { name: "Canvas zoom preset" })).toHaveValue("fit");
  });

  it("zooms on trackpad pinch or modified mouse wheel without consuming scroll", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const viewport = screen.getByLabelText("Screenshot editing canvas");
    const zoom = setCanvasZoomPercent(100);

    const scroll = new WheelEvent("wheel", {
      deltaY: -100,
      bubbles: true,
      cancelable: true,
    });
    fireEvent(viewport, scroll);
    expect(scroll.defaultPrevented).toBe(false);
    expect(canvasZoomPercent(zoom)).toBeCloseTo(100, 0);

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
    expect(canvasZoomPercent(zoom)).toBeGreaterThan(100);

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
    expect(canvasZoomPercent(zoom)).toBeCloseTo(100, 1);
  });

  it("supports the native macOS magnify gesture", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const viewport = screen.getByLabelText("Screenshot editing canvas");
    const zoom = setCanvasZoomPercent(100);

    const start = new Event("gesturestart", { bubbles: true, cancelable: true });
    Object.assign(start, { clientX: 300, clientY: 200, scale: 1 });
    fireEvent(viewport, start);
    expect(start.defaultPrevented).toBe(true);

    const change = new Event("gesturechange", { bubbles: true, cancelable: true });
    Object.assign(change, { clientX: 300, clientY: 200, scale: 1.5 });
    fireEvent(viewport, change);
    expect(change.defaultPrevented).toBe(true);
    expect(canvasZoomPercent(zoom)).toBeCloseTo(150, 0);

    const end = new Event("gestureend", { bubbles: true, cancelable: true });
    fireEvent(viewport, end);
    expect(end.defaultPrevented).toBe(true);
  });

  it("pans the canvas with Command/Ctrl-drag from the canvas surface", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const viewport = screen.getByLabelText("Screenshot editing canvas");
    const canvas = viewport.querySelector("canvas")!;
    const surface = viewport.querySelector(".screenshot-canvas-surface") as HTMLElement;
    viewport.setPointerCapture = vi.fn();
    viewport.hasPointerCapture = vi.fn(() => true);
    viewport.releasePointerCapture = vi.fn();

    // Space no longer arms pan mode.
    fireEvent.keyDown(document.body, { code: "Space", key: " " });
    expect(viewport).not.toHaveClass("is-pan-ready");

    const metaDown = new KeyboardEvent("keydown", {
      code: "MetaLeft",
      key: "Meta",
      metaKey: true,
      bubbles: true,
      cancelable: true,
    });
    fireEvent(document.body, metaDown);
    expect(viewport).toHaveClass("is-pan-ready");

    // Drag starting on the canvas (not only empty viewport chrome).
    fireEvent.pointerDown(canvas, {
      button: 0,
      buttons: 1,
      pointerId: 10,
      clientX: 400,
      clientY: 300,
      metaKey: true,
    });
    expect(viewport).toHaveClass("is-panning");
    fireEvent.pointerMove(viewport, {
      buttons: 1,
      pointerId: 10,
      clientX: 350,
      clientY: 260,
      metaKey: true,
    });
    expect(surface.style.transform).toBe("translate(-50px, -40px)");
    fireEvent.pointerUp(viewport, { button: 0, pointerId: 10, metaKey: true });
    expect(viewport).not.toHaveClass("is-panning");
    fireEvent.keyUp(document.body, { code: "MetaLeft", key: "Meta" });
    expect(viewport).not.toHaveClass("is-pan-ready");

    // Ctrl-drag continues panning from the current free-pan offset (Windows/Linux).
    fireEvent.pointerDown(canvas, {
      button: 0,
      buttons: 1,
      pointerId: 11,
      clientX: 300,
      clientY: 220,
      ctrlKey: true,
    });
    fireEvent.pointerMove(viewport, {
      buttons: 1,
      pointerId: 11,
      clientX: 270,
      clientY: 200,
      ctrlKey: true,
    });
    expect(surface.style.transform).toBe("translate(-80px, -60px)");
    fireEvent.pointerUp(viewport, { button: 0, pointerId: 11, ctrlKey: true });
  });

  it("fades in Recenter when the canvas is off-screen and restores it", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const viewport = screen.getByLabelText("Screenshot editing canvas");
    const canvas = viewport.querySelector("canvas")!;
    const surface = viewport.querySelector(".screenshot-canvas-surface") as HTMLElement;
    viewport.setPointerCapture = vi.fn();
    viewport.hasPointerCapture = vi.fn(() => true);
    viewport.releasePointerCapture = vi.fn();

    vi.spyOn(viewport, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 800,
      bottom: 600,
      width: 800,
      height: 600,
      toJSON: () => ({}),
    });
    const surfaceBounds = vi.spyOn(surface, "getBoundingClientRect");
    surfaceBounds.mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 400,
      bottom: 300,
      width: 400,
      height: 300,
      toJSON: () => ({}),
    });

    // Still mostly on-screen — no cue.
    fireEvent.scroll(viewport);
    expect(screen.queryByRole("button", { name: "Recenter" })).toBeNull();

    // Pan far enough that the surface no longer intersects the viewport.
    surfaceBounds.mockReturnValue({
      x: 2_000,
      y: 2_000,
      top: 2_000,
      left: 2_000,
      right: 2_400,
      bottom: 2_300,
      width: 400,
      height: 300,
      toJSON: () => ({}),
    });
    fireEvent.pointerDown(canvas, {
      button: 0,
      buttons: 1,
      pointerId: 20,
      clientX: 100,
      clientY: 100,
      metaKey: true,
    });
    fireEvent.pointerMove(viewport, {
      buttons: 1,
      pointerId: 20,
      clientX: 100 + 2_000,
      clientY: 100 + 2_000,
      metaKey: true,
    });
    fireEvent.pointerUp(viewport, { button: 0, pointerId: 20, metaKey: true });

    const recenter = await screen.findByRole("button", { name: "Recenter" });
    expect(recenter).toHaveClass("is-visible");
    expect(surface.style.transform).toBe("translate(2000px, 2000px)");

    surfaceBounds.mockReturnValue({
      x: 200,
      y: 150,
      top: 150,
      left: 200,
      right: 600,
      bottom: 450,
      width: 400,
      height: 300,
      toJSON: () => ({}),
    });
    fireEvent.click(recenter);
    expect(surface.style.transform).toBe("translate(0px, 0px)");
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Recenter" })).toBeNull();
    });
  });

  it("creates selectable formatted text directly on the canvas", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
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
    fireEvent.pointerUp(canvas, {
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
    const initialWidth = Number.parseFloat(inlineEditor.style.width);
    fireEvent.change(inlineEditor, {
      target: { value: "Hello from the screenshot editor" },
    });
    expect(screen.getByRole("textbox", { name: "Text" })).toHaveValue(
      "Hello from the screenshot editor",
    );
    expect(Number.parseFloat(inlineEditor.style.width)).toBeGreaterThan(initialWidth);
    expect(inlineEditor).toHaveClass("is-auto-width");
    expect(inlineEditor).toHaveAttribute("wrap", "off");
    expect(screen.getByRole("button", { name: "Bold" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Italic" })).toBeInTheDocument();
    expect(
      within(screen.getByRole("region", { name: "Layers" })).getByText(
        "Hello from the screenshot editor",
      ),
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
      .toHaveValue("Hello from the screenshot editor");
  });

  it("creates text with any of the seven visual style presets", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
    fireEvent.click(screen.getByRole("button", { name: "Text (T)" }));
    // Rounded Box is the default for new text (bubbly label style).
    fireEvent.click(screen.getByRole("button", { name: "New text style: Rounded Box" }));

    for (const style of [
      "Standard",
      "Rounded",
      "Outlined",
      "Mono",
      "Box",
      "Mono Box",
      "Rounded Box",
    ]) {
      expect(screen.getByRole("menuitemradio", { name: style }))
        .toBeInTheDocument();
    }

    fireEvent.click(screen.getByRole("menuitemradio", { name: "Rounded Box" }));
    expect(screen.getByRole("button", { name: "New text style: Rounded Box" }))
      .toHaveAttribute("aria-expanded", "false");

    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    setCanvasBounds(canvas);
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 31,
      clientX: 120,
      clientY: 80,
    });

    const inlineEditor = await screen.findByRole("textbox", {
      name: "Edit text on canvas",
    });
    expect(inlineEditor.style.fontFamily).toContain("ui-rounded");
    expect(inlineEditor).toHaveStyle({ backgroundColor: "#111318" });
    expect(Number.parseFloat(inlineEditor.style.borderRadius)).toBeGreaterThan(20);
    expect(inlineEditor).toHaveStyle({ textAlign: "center" });
    expect(screen.getByRole("button", { name: "Text style: Rounded Box" }))
      .toHaveAttribute("aria-expanded", "false");

    fireEvent.click(screen.getByRole("button", { name: "Text style: Rounded Box" }));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("menu", { name: "Text style" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Text style: Rounded Box" }))
      .toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Text style: Rounded Box" }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Outlined" }));
    expect(screen.getByRole("button", { name: "Text style: Outlined" }))
      .toHaveAttribute("aria-expanded", "false");
    expect(inlineEditor.style.color).toBe("transparent");
    expect(inlineEditor.style.backgroundColor).toBe("transparent");
    expect(inlineEditor.style.webkitTextStroke).toContain("#ff3b5c");
  });

  it("copies, pastes, and duplicates the selected layer with standard shortcuts", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    fireEvent.click(screen.getByRole("button", { name: "Text (T)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    setCanvasBounds(canvas);
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 20,
      clientX: 120,
      clientY: 80,
    });
    fireEvent.pointerUp(canvas, {
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

  it("draws one straight Arrow and bends it from one of three starter dots", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
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
    const arrowLayer = within(layers).getByRole("button", { name: /ArrowShape/ });
    // Shape layers paint a live canvas thumbnail (color/geometry), not a static icon.
    expect(
      arrowLayer.querySelector("canvas.screenshot-layer-preview-canvas"),
    ).toBeTruthy();
    fireEvent.click(arrowLayer);
    const curve = screen.getByRole("slider", { name: "Curve" });
    expect(curve).toHaveValue("0");
    expect(screen.getByText(/Drag the curve dots to reshape/)).toBeInTheDocument();

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
    expect(screen.getByRole("button", { name: "Straighten arrow" })).toBeInTheDocument();
  });

  it("shrinks the arrow head when the tip handle is dragged back", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
    fireEvent.click(screen.getByRole("button", { name: "Arrow (A)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    canvas.setPointerCapture = vi.fn();
    canvas.hasPointerCapture = vi.fn(() => true);
    canvas.releasePointerCapture = vi.fn();
    setCanvasBounds(canvas);

    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 70,
      clientX: 80,
      clientY: 200,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 70,
      clientX: 480,
      clientY: 200,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 70,
      clientX: 480,
      clientY: 200,
    });

    const stroke = screen.getByRole("slider", { name: "Stroke width" });
    expect(stroke).toHaveAttribute("aria-valuetext", "8 px");

    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 71,
      clientX: 480,
      clientY: 200,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 71,
      clientX: 160,
      clientY: 200,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 71,
      clientX: 160,
      clientY: 200,
    });

    const shortened = screen.getByRole("slider", { name: "Stroke width" });
    const label = shortened.getAttribute("aria-valuetext") ?? "";
    const px = Number(label.replace(" px", ""));
    expect(px).toBeGreaterThan(0);
    expect(px).toBeLessThan(4);
  });

  it("shrinks the arrow head when a box-corner grip scales the arrow down", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
    fireEvent.click(screen.getByRole("button", { name: "Arrow (A)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    canvas.setPointerCapture = vi.fn();
    canvas.hasPointerCapture = vi.fn(() => true);
    canvas.releasePointerCapture = vi.fn();
    setCanvasBounds(canvas);

    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 80,
      clientX: 80,
      clientY: 200,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 80,
      clientX: 480,
      clientY: 200,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 80,
      clientX: 480,
      clientY: 200,
    });

    expect(screen.getByRole("slider", { name: "Stroke width" }))
      .toHaveAttribute("aria-valuetext", "8 px");

    const placed: EditorShapeElement = {
      id: "placed-arrow",
      kind: "shape",
      shape: "arrow",
      x: 80,
      y: 200,
      endX: 480,
      endY: 200,
      controls: [],
      style: { color: "#ff3b5c", fill: null, strokeWidth: 8 },
      locked: false,
      visible: true,
      opacity: 100,
      blendMode: "source-over",
    };
    const bounds = elementBounds(placed);
    const se = resizeHandlePoint(bounds, "se");
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 81,
      clientX: se.x,
      clientY: se.y,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 81,
      clientX: bounds.x + bounds.width * 0.28,
      clientY: bounds.y + bounds.height * 0.28,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 81,
      clientX: bounds.x + bounds.width * 0.28,
      clientY: bounds.y + bounds.height * 0.28,
    });

    const scaled = screen.getByRole("slider", { name: "Stroke width" });
    const scaledPx = Number((scaled.getAttribute("aria-valuetext") ?? "").replace(" px", ""));
    expect(scaledPx).toBeGreaterThan(0);
    expect(scaledPx).toBeLessThan(4);
  });

  it("shows curve handles after placing a stroke and bends without leaving the shape tool", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
    fireEvent.click(screen.getByRole("button", { name: "Arrow (A)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    canvas.setPointerCapture = vi.fn();
    canvas.hasPointerCapture = vi.fn(() => true);
    canvas.releasePointerCapture = vi.fn();
    setCanvasBounds(canvas);

    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 60,
      clientX: 100,
      clientY: 150,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 60,
      clientX: 300,
      clientY: 150,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 60,
      clientX: 300,
      clientY: 150,
    });

    // Post-place selection: Curve controls appear while Arrow remains active.
    expect(screen.getByRole("button", { name: "Arrow (A)" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    const curve = screen.getByRole("slider", { name: "Curve" });
    expect(curve).toHaveValue("0");
    expect(screen.getByRole("button", { name: /ArrowShape/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    // Drag the middle starter dot without switching to Select & move.
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 61,
      clientX: 200,
      clientY: 150,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 61,
      clientX: 200,
      clientY: 250,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 61,
      clientX: 200,
      clientY: 250,
    });
    expect(screen.getByRole("button", { name: "Straighten arrow" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Arrow (A)" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    // Empty canvas still starts a second arrow while the tool stays active.
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 62,
      clientX: 80,
      clientY: 400,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 62,
      clientX: 220,
      clientY: 400,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 62,
      clientX: 220,
      clientY: 400,
    });
    const layers = screen.getByRole("region", { name: "Layers" });
    expect(within(layers).getAllByRole("button", { name: /ArrowShape/ })).toHaveLength(2);
  });

  it("lets a selected drawing toggle a drop shadow", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    fireEvent.click(screen.getByRole("button", { name: "Arrow (A)" }));
    const defaults = screen.getByRole("checkbox", { name: "Drop shadow" });
    expect(defaults).not.toBeChecked();

    setCanvasZoomPercent(100);
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    canvas.setPointerCapture = vi.fn();
    canvas.hasPointerCapture = vi.fn(() => true);
    canvas.releasePointerCapture = vi.fn();
    setCanvasBounds(canvas);

    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 90,
      clientX: 120,
      clientY: 180,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 90,
      clientX: 360,
      clientY: 180,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 90,
      clientX: 360,
      clientY: 180,
    });

    const shadow = screen.getByRole("checkbox", { name: "Drop shadow" });
    expect(shadow).not.toBeChecked();
    fireEvent.click(shadow);
    expect(shadow).toBeChecked();
  });

  it("adds and removes uncapped arrow curve points without creating arrows on double-click", async () => {
    render(<ScreenshotEditor />);
    await screen.findAllByText("1440 × 900");

    setCanvasZoomPercent(100);
    fireEvent.click(screen.getByRole("button", { name: "Arrow (A)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    canvas.setPointerCapture = vi.fn();
    canvas.hasPointerCapture = vi.fn(() => true);
    canvas.releasePointerCapture = vi.fn();
    setCanvasBounds(canvas);
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 40,
      clientX: 100,
      clientY: 200,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 40,
      clientX: 400,
      clientY: 200,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 40,
      clientX: 400,
      clientY: 200,
    });

    const layers = screen.getByRole("region", { name: "Layers" });
    expect(within(layers).getAllByRole("button", { name: /ArrowShape/ })).toHaveLength(1);
    expect(screen.getByText(/Double-click the path to add more points/)).toBeInTheDocument();
    expect(screen.queryByText(/\d+\/4/)).not.toBeInTheDocument();

    const clickBeforeDoubleClick = (pointerId: number, clientX: number) => {
      fireEvent.pointerDown(canvas, {
        button: 0,
        pointerId,
        clientX,
        clientY: 200,
      });
      fireEvent.pointerUp(canvas, {
        button: 0,
        pointerId,
        clientX,
        clientY: 200,
      });
    };

    // Reproduce the browser's two pointer click cycles before `dblclick`.
    // These used to create two tiny arrows before the curve point was inserted.
    clickBeforeDoubleClick(41, 210);
    clickBeforeDoubleClick(42, 210);
    fireEvent.doubleClick(canvas, {
      button: 0,
      clientX: 210,
      clientY: 200,
    });
    expect(within(layers).getAllByRole("button", { name: /ArrowShape/ })).toHaveLength(1);
    expect(screen.getByRole("slider", { name: "Curve" })).toBeInTheDocument();

    // A second point is inserted with the Arrow tool still active and no cap copy.
    clickBeforeDoubleClick(43, 340);
    clickBeforeDoubleClick(44, 340);
    fireEvent.doubleClick(canvas, {
      button: 0,
      clientX: 340,
      clientY: 200,
    });
    expect(screen.getByRole("button", { name: "Straighten arrow" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Arrow (A)" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(within(layers).getAllByRole("button", { name: /ArrowShape/ })).toHaveLength(1);

    // Double-click an existing point to remove it, still without switching tools.
    clickBeforeDoubleClick(45, 210);
    clickBeforeDoubleClick(46, 210);
    fireEvent.doubleClick(canvas, {
      button: 0,
      clientX: 210,
      clientY: 200,
    });
    expect(screen.getByRole("slider", { name: "Curve" })).toBeInTheDocument();
    expect(within(layers).getAllByRole("button", { name: /ArrowShape/ })).toHaveLength(1);
  });

  it("curves lines with the same handles, double-click points, and hover tip", async () => {
    render(<ScreenshotEditor />);
    await screen.findAllByText("1440 × 900");

    setCanvasZoomPercent(100);
    fireEvent.click(screen.getByRole("button", { name: "Line (L)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    canvas.setPointerCapture = vi.fn();
    canvas.hasPointerCapture = vi.fn(() => true);
    canvas.releasePointerCapture = vi.fn();
    setCanvasBounds(canvas);
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 50,
      clientX: 100,
      clientY: 300,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 50,
      clientX: 400,
      clientY: 300,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 50,
      clientX: 400,
      clientY: 300,
    });

    const layers = screen.getByRole("region", { name: "Layers" });
    fireEvent.click(within(layers).getByRole("button", { name: /LineShape/ }));
    expect(screen.getByRole("slider", { name: "Curve" })).toBeInTheDocument();
    expect(screen.getByText(/Double-click the path to add more points/)).toBeInTheDocument();

    // Middle starter-dot bend materializes the three editable curve points.
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 51,
      clientX: 250,
      clientY: 300,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 51,
      clientX: 250,
      clientY: 450,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 51,
      clientX: 250,
      clientY: 450,
    });
    expect(screen.getByRole("button", { name: "Straighten line" })).toBeInTheDocument();

    // Hover the path → discovery tooltip (fixed to viewport, not the panned surface).
    fireEvent.pointerMove(canvas, {
      clientX: 320,
      clientY: 360,
    });
    const tip = screen.getByRole("tooltip", {
      name: /Double-click to add a curve point/,
    });
    expect(tip).toBeInTheDocument();
    expect(tip).toHaveClass("screenshot-curve-hover-tip");
    expect(tip).toHaveStyle({ left: "320px", top: "360px" });
    expect(tip.closest(".screenshot-canvas-surface")).toBeNull();

    fireEvent.doubleClick(canvas, {
      button: 0,
      clientX: 340,
      clientY: 360,
    });
    expect(screen.getByRole("button", { name: "Straighten line" })).toBeInTheDocument();
  });

  it("renames image layers inline and keeps secondary controls in the layer popover", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const layers = screen.getByRole("region", { name: "Layers" });
    const originalLayer = within(layers).getByRole("button", {
      name: /Original screenshotLocked background/,
    });

    // Selecting a layer no longer reserves the inspector for rarely used settings.
    fireEvent.click(originalLayer);
    expect(screen.queryByRole("slider", { name: "Layer opacity" })).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Blend mode")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Layer name")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete selected item" })).not.toBeInTheDocument();

    fireEvent.doubleClick(originalLayer);
    const rename = within(layers).getByRole("textbox", { name: "Rename layer" });
    expect(rename).toHaveValue("Original screenshot");
    fireEvent.change(rename, { target: { value: "Reference image" } });
    fireEvent.keyDown(rename, { key: "Enter" });
    expect(within(layers).getByRole("button", {
      name: /Reference imageLocked background/,
    })).toBeInTheDocument();

    // Lock and visibility stay as the only always-visible layer-row actions.
    const layerLock = within(layers).getByRole("button", {
      name: "Unlock Reference image",
    });
    expect(layerLock).toHaveAttribute("aria-pressed", "true");
    expect(layerLock).toHaveClass("active");
    expect(
      within(layers).queryByRole("button", { name: "Lock Reference image" }),
    ).not.toBeInTheDocument();

    fireEvent.click(layerLock);
    expect(screen.queryByRole("button", { name: "Bring to front" })).not.toBeInTheDocument();
    const layerSettingsTrigger = within(layers).getByRole("button", {
      name: "Layer settings for Reference image",
    });
    fireEvent.click(layerSettingsTrigger);
    const layerSettings = screen.getByRole("dialog", {
      name: "Layer settings for Reference image",
    });
    expect(within(layerSettings).getByRole("slider", { name: "Layer opacity" }))
      .toHaveValue("100");
    expect(within(layerSettings).getByRole("combobox", { name: "Blend mode" }))
      .toHaveTextContent("Normal");
    fireEvent.change(within(layerSettings).getByRole("slider", { name: "Layer opacity" }), {
      target: { value: "65" },
    });
    expect(within(layerSettings).getByRole("slider", { name: "Layer opacity" }))
      .toHaveAttribute("aria-valuetext", "65%");
    expect(within(layerSettings).getByRole("button", { name: "Duplicate" })).toBeEnabled();
    expect(within(layerSettings).getByRole("button", { name: "Delete" })).toBeEnabled();
    // Single-layer document: nothing to merge into, and flatten only bakes when
    // a solid canvas background remains (it does by default).
    expect(within(layerSettings).getByRole("button", { name: "Merge down" })).toBeDisabled();
    expect(within(layerSettings).getByRole("button", { name: "Merge visible" })).toBeDisabled();
    expect(within(layerSettings).getByRole("button", { name: "Flatten image" })).toBeEnabled();
    expect(within(layerSettings).getByRole("button", { name: "Bring to front" })).toBeDisabled();
    expect(within(layerSettings).getByRole("button", { name: "Send to back" })).toBeDisabled();
    expect(
      within(layers).getByRole("button", { name: "Lock Reference image" }),
    ).toHaveAttribute("aria-pressed", "false");
    expect(within(layers).getByText("Background")).toBeInTheDocument();
  });

  it("resizes the canvas once per entry so undo restores the previous size", async () => {
    render(<ScreenshotEditor />);
    const canvasWidth = await screen.findByLabelText("Canvas width");

    for (const text of ["1", "14", "140", "1400"]) {
      fireEvent.change(canvasWidth, { target: { value: text } });
    }
    expect(canvasWidth).toHaveValue(1400);
    fireEvent.keyDown(canvasWidth, { key: "Enter" });
    expect(canvasWidth).toHaveValue(1400);

    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(canvasWidth).toHaveValue(1440);
    expect(screen.getByRole("button", { name: "Undo" })).toBeDisabled();
  });

  it("rotates and flips a locked image layer with undo support", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const layers = screen.getByRole("region", { name: "Layers" });
    const originalLayer = within(layers).getByRole("button", {
      name: /Original screenshotLocked background/,
    });
    fireEvent.click(within(layers).getByRole("button", {
      name: "Layer settings for Original screenshot",
    }));
    const transforms = screen.getByRole("group", {
      name: "Image transforms for Original screenshot",
    });
    expect(within(transforms).getByRole("button", {
      name: "Rotate image counterclockwise",
    })).toBeEnabled();
    expect(within(transforms).getByRole("button", {
      name: "Rotate image clockwise",
    })).toBeEnabled();
    expect(within(transforms).getByRole("button", {
      name: "Flip image horizontally",
    })).toBeEnabled();
    expect(within(transforms).getByRole("button", {
      name: "Flip image vertically",
    })).toBeEnabled();

    const preview = originalLayer.querySelector("img")!;
    fireEvent.click(within(transforms).getByRole("button", {
      name: "Flip image horizontally",
    }));
    expect(preview.style.transform).toBe("matrix(-1, 0, 0, 1, 0, 0)");

    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(preview.style.transform).toBe("matrix(1, 0, 0, 1, 0, 0)");

    fireEvent.click(within(transforms).getByRole("button", {
      name: "Rotate image clockwise",
    }));
    const canvasToolbar = screen.getByRole("group", { name: "Canvas" });
    expect(within(canvasToolbar).getByLabelText("Canvas width")).toHaveValue(900);
    expect(within(canvasToolbar).getByLabelText("Canvas height")).toHaveValue(1440);
    expect(preview.style.transform).toBe("matrix(0, 1, -1, 0, 0, 0)");

    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(within(canvasToolbar).getByLabelText("Canvas width")).toHaveValue(1440);
    expect(within(canvasToolbar).getByLabelText("Canvas height")).toHaveValue(900);
  });

  it("exposes eraser modes and wand controls", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    fireEvent.click(screen.getByRole("button", { name: "Eraser (B)" }));
    expect(screen.getByRole("button", { name: "Wand" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText("Color tolerance")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Contiguous only" })).toBeChecked();
    expect(
      screen.getByText(/Make pixels transparent on image layers/i),
    ).toBeInTheDocument();
    expect(screen.getByText(/Not automatic subject cutout/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Erase" }));
    expect(screen.getByRole("button", { name: "Erase" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText("Brush size")).toBeInTheDocument();
    expect(screen.queryByLabelText("Color tolerance")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Restore" }));
    expect(screen.getByRole("button", { name: "Restore" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText("Brush size")).toBeInTheDocument();
  });

  it("shows a size-matched circular brush cursor for erase mode", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
    fireEvent.click(screen.getByRole("button", { name: "Eraser (B)" }));
    fireEvent.click(screen.getByRole("button", { name: "Erase" }));

    const canvas = document.querySelector("canvas.screenshot-canvas");
    expect(canvas).toBeInstanceOf(HTMLCanvasElement);
    setCanvasBounds(canvas as HTMLCanvasElement, 1_440, 900);

    // Hover the center of the original image layer (full-canvas artifact).
    fireEvent.pointerMove(canvas as HTMLCanvasElement, {
      clientX: 400,
      clientY: 300,
      pointerId: 1,
    });

    const ring = await waitFor(() => {
      const el = document.querySelector(".screenshot-brush-cursor");
      expect(el).toBeInstanceOf(HTMLElement);
      return el as HTMLElement;
    });
    expect(ring).toHaveClass("is-erase");
    // Default brush size is 28 document px at 100% zoom → 28 CSS px diameter.
    expect(ring).toHaveStyle({ width: "28px", height: "28px", left: "400px", top: "300px" });

    // Changing brush size resizes the ring without another pointer move.
    fireEvent.change(screen.getByLabelText("Brush size"), { target: { value: "64" } });
    await waitFor(() => {
      const next = document.querySelector(".screenshot-brush-cursor") as HTMLElement;
      expect(next).toHaveStyle({ width: "64px", height: "64px" });
    });

    fireEvent.click(screen.getByRole("button", { name: "Wand" }));
    await waitFor(() => {
      expect(document.querySelector(".screenshot-brush-cursor")).toBeNull();
    });
  });

  it("shows a magnified color loupe while hovering with the remove-bg wand", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
    fireEvent.click(screen.getByRole("button", { name: "Eraser (B)" }));
    expect(screen.getByRole("button", { name: "Wand" })).toHaveAttribute("aria-pressed", "true");

    const canvas = document.querySelector("canvas.screenshot-canvas");
    expect(canvas).toBeInstanceOf(HTMLCanvasElement);
    setCanvasBounds(canvas as HTMLCanvasElement, 1_440, 900);

    // Hover the center of the original image layer (full-canvas artifact).
    fireEvent.pointerMove(canvas as HTMLCanvasElement, {
      clientX: 400,
      clientY: 300,
      pointerId: 1,
    });

    const loupe = await waitFor(() => {
      const el = document.querySelector(".screenshot-wand-loupe");
      expect(el).toBeInstanceOf(HTMLElement);
      return el as HTMLElement;
    });
    expect(loupe).toHaveAttribute("role", "tooltip");
    expect(loupe.querySelector(".screenshot-wand-loupe-canvas")).toBeInstanceOf(HTMLCanvasElement);

    // Leaving the canvas hides the loupe until the next hover.
    fireEvent.pointerLeave(canvas as HTMLCanvasElement);
    await waitFor(() => {
      expect(document.querySelector(".screenshot-wand-loupe")).toBeNull();
    });

    // Erase mode uses the brush ring, not the wand loupe.
    fireEvent.pointerMove(canvas as HTMLCanvasElement, {
      clientX: 400,
      clientY: 300,
      pointerId: 1,
    });
    await waitFor(() => {
      expect(document.querySelector(".screenshot-wand-loupe")).toBeInstanceOf(HTMLElement);
    });
    fireEvent.click(screen.getByRole("button", { name: "Erase" }));
    await waitFor(() => {
      expect(document.querySelector(".screenshot-wand-loupe")).toBeNull();
    });
  });

  it("coalesces erase paints and keeps the committed bitmap decoded on release", async () => {
    const brushArtifact = { ...artifact, width: 20, height: 10 };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifact") return brushArtifact;
      const draft = draftCommandResult(String(command));
      if (draft !== undefined || String(command).includes("screenshot_editor_draft")) {
        return draft;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    const operations: string[] = [];
    const putImageData = vi.fn();
    const context = {
      clearRect: vi.fn(() => operations.push("clear")),
      fillRect: vi.fn(),
      drawImage: vi.fn((source: CanvasImageSource) => {
        operations.push(source instanceof HTMLCanvasElement ? "draw-canvas" : "draw-image");
      }),
      getImageData: vi.fn((_x: number, _y: number, width: number, height: number) => {
        const data = new Uint8ClampedArray(width * height * 4);
        for (let index = 3; index < data.length; index += 4) data[index] = 255;
        return { data, width, height, colorSpace: "srgb" } as ImageData;
      }),
      putImageData,
      save: vi.fn(),
      restore: vi.fn(),
      translate: vi.fn(),
      transform: vi.fn(),
      setLineDash: vi.fn(),
      strokeRect: vi.fn(),
      imageSmoothingEnabled: true,
      imageSmoothingQuality: "high",
    } as unknown as CanvasRenderingContext2D;
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(context);
    vi.spyOn(HTMLCanvasElement.prototype, "toDataURL")
      .mockReturnValue("data:image/png;base64,edited");

    const imageSources: string[] = [];
    const originalImage = window.Image;
    class LoadedImage {
      onload: ((event: Event) => void) | null = null;
      onerror: ((event: Event) => void) | null = null;
      naturalWidth = brushArtifact.width;
      naturalHeight = brushArtifact.height;
      width = brushArtifact.width;
      height = brushArtifact.height;
      crossOrigin = "";
      set src(value: string) {
        imageSources.push(value);
        if (!value.startsWith("data:")) {
          queueMicrotask(() => this.onload?.(new Event("load")));
        }
      }
    }
    // @ts-expect-error focused Image decode stub
    window.Image = LoadedImage;

    try {
      render(<ScreenshotEditor />);
      await screen.findByLabelText("Canvas width");
      fireEvent.click(screen.getByRole("button", { name: "Eraser (B)" }));
      fireEvent.click(screen.getByRole("button", { name: "Erase" }));
      fireEvent.change(screen.getByLabelText("Brush size"), { target: { value: "4" } });

      const canvas = document.querySelector("canvas.screenshot-canvas") as HTMLCanvasElement;
      setCanvasBounds(canvas, brushArtifact.width, brushArtifact.height);
      fireEvent.pointerDown(canvas, {
        button: 0,
        clientX: 2,
        clientY: 5,
        pointerId: 7,
      });

      const frames: FrameRequestCallback[] = [];
      vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
        frames.push(callback);
        return frames.length;
      });
      putImageData.mockClear();
      fireEvent.pointerMove(canvas, { clientX: 4, clientY: 5, pointerId: 7 });
      fireEvent.pointerMove(canvas, { clientX: 8, clientY: 5, pointerId: 7 });

      expect(frames).toHaveLength(1);
      expect(putImageData).not.toHaveBeenCalled();
      act(() => frames.shift()?.(0));
      expect(putImageData).toHaveBeenCalledOnce();
      expect(putImageData.mock.calls[0]).toHaveLength(7);
      expect(putImageData.mock.calls[0]?.slice(3)).toEqual([0, 3, 11, 5]);

      operations.length = 0;
      fireEvent.pointerUp(canvas, { clientX: 8, clientY: 5, pointerId: 7 });
      await waitFor(() => {
        expect(screen.getByRole("button", { name: "Undo" })).toBeEnabled();
      });

      // The committed data URL is backed by the working canvas immediately;
      // no second Image decode can clear the editor while it is still loading.
      expect(imageSources).not.toContain("data:image/png;base64,edited");
      const lastClear = operations.lastIndexOf("clear");
      expect(lastClear).toBeGreaterThanOrEqual(0);
      expect(operations.slice(lastClear + 1)).toContain("draw-canvas");
    } finally {
      window.Image = originalImage;
    }
  });

  it("can clear the solid canvas background for transparent PNG/WebP exports", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    // Canvas background lives behind a compact header button so the swatches
    // do not occupy the toolbar until the picker is opened.
    const canvasToolbar = screen.getByRole("group", { name: "Canvas" });
    const backgroundTrigger = within(canvasToolbar).getByRole("button", {
      name: /Background color/,
    });
    expect(backgroundTrigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("dialog", { name: "Canvas background" })).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/Canvas background:/)).not.toBeInTheDocument();

    fireEvent.click(backgroundTrigger);
    const picker = await screen.findByRole("dialog", { name: "Canvas background" });
    expect(backgroundTrigger).toHaveAttribute("aria-expanded", "true");
    const solidBackground = within(picker).getByRole("checkbox", {
      name: "Solid background",
    });
    expect(solidBackground).toBeChecked();
    expect(within(picker).getByLabelText("Canvas background: #ffffff")).toBeInTheDocument();

    const surface = screen
      .getByLabelText("Screenshot editing canvas")
      .querySelector(".screenshot-canvas-surface");
    expect(surface).not.toHaveClass("transparent");

    fireEvent.click(solidBackground);
    expect(solidBackground).not.toBeChecked();
    expect(surface).toHaveClass("transparent");
    expect(backgroundTrigger).toHaveAccessibleName("Background color: transparent");

    fireEvent.click(solidBackground);
    expect(solidBackground).toBeChecked();
    expect(within(picker).getByLabelText("Canvas background: #ffffff")).toBeInTheDocument();
    expect(surface).not.toHaveClass("transparent");
    expect(backgroundTrigger).toHaveAccessibleName(/Background color: #f7f7f5/i);
  });

  it("picks a canvas background color from the compact picker popover", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const canvasToolbar = screen.getByRole("group", { name: "Canvas" });
    fireEvent.click(within(canvasToolbar).getByRole("button", { name: /Background color/ }));
    const picker = await screen.findByRole("dialog", { name: "Canvas background" });
    const solidBackground = within(picker).getByRole("checkbox", {
      name: "Solid background",
    });

    fireEvent.click(solidBackground);
    expect(solidBackground).not.toBeChecked();

    fireEvent.click(within(picker).getByLabelText("Canvas background: #ff3b5c"));

    expect(solidBackground).toBeChecked();
    const surface = screen
      .getByLabelText("Screenshot editing canvas")
      .querySelector(".screenshot-canvas-surface");
    expect(surface).not.toHaveClass("transparent");
    expect(surface).toHaveStyle({ backgroundColor: "#ff3b5c" });
    expect(within(canvasToolbar).getByRole("button", { name: /Background color/ }))
      .toHaveAccessibleName("Background color: #ff3b5c");

    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Canvas background" })).not.toBeInTheDocument();
    });
  });

  it("offers Trim edges in the header canvas toolbar and disables it when already tight", async () => {
    render(<ScreenshotEditor />);
    const widthInput = await screen.findByLabelText("Canvas width");
    const canvasToolbar = screen.getByRole("group", { name: "Canvas" });

    // Fresh capture fills the canvas — nothing to trim.
    const canvasTrim = within(canvasToolbar).getByRole("button", { name: "Trim edges" });
    expect(canvasTrim.querySelector("svg")).not.toBeNull();
    expect(canvasTrim).toBeDisabled();

    // Manual canvas growth creates empty margin; trim should re-enable.
    fireEvent.change(widthInput, { target: { value: "1600" } });
    fireEvent.keyDown(widthInput, { key: "Enter" });
    await waitFor(() => {
      expect(within(canvasToolbar).getByRole("button", { name: "Trim edges" })).toBeEnabled();
    });

    fireEvent.click(within(canvasToolbar).getByRole("button", { name: "Trim edges" }));
    await waitFor(() => {
      expect(screen.getByLabelText("Canvas width")).toHaveValue(1440);
      expect(within(canvasToolbar).getByRole("button", { name: "Trim edges" })).toBeDisabled();
    });
  });

  it("previews margins that Trim edges would remove while hovering the control", async () => {
    render(<ScreenshotEditor />);
    const widthInput = await screen.findByLabelText("Canvas width");
    const canvasToolbar = screen.getByRole("group", { name: "Canvas" });
    const canvas = screen.getByLabelText("Screenshot editing canvas");

    fireEvent.change(widthInput, { target: { value: "1600" } });
    fireEvent.keyDown(widthInput, { key: "Enter" });
    const canvasTrim = within(canvasToolbar).getByRole("button", { name: "Trim edges" });
    await waitFor(() => {
      expect(canvasTrim).toBeEnabled();
    });

    expect(canvas.querySelector(".screenshot-canvas-trim-hint")).toBeNull();

    fireEvent.pointerEnter(canvasTrim);
    await waitFor(() => {
      const hint = canvas.querySelector(".screenshot-canvas-trim-hint");
      expect(hint).not.toBeNull();
      expect(hint?.querySelector(".screenshot-canvas-trim-region.edge-right")).not.toBeNull();
      expect(hint?.querySelectorAll(".screenshot-canvas-trim-particle").length).toBeGreaterThan(0);
    });

    fireEvent.pointerLeave(canvasTrim);
    await waitFor(() => {
      expect(canvas.querySelector(".screenshot-canvas-trim-hint")).toBeNull();
    });
  });

  it("selects a newly drawn shape so handles are ready without switching tools", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

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

    // Shape tool stays active, but the new rectangle is selected for post-place edits.
    expect(screen.getByRole("button", { name: "Rectangle (R)" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(
      within(screen.getByRole("region", { name: "Layers" })).getByRole("button", {
        name: /RectangleShape/,
      }),
    ).toHaveAttribute("aria-pressed", "true");
  });

  it("deselects the active layer when clicking the empty viewport chrome", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
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
    fireEvent.pointerUp(canvas, {
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
    const textLayer = within(screen.getByRole("region", { name: "Layers" }))
      .getByRole("button", { name: /TextText/ });
    expect(textLayer).toHaveAttribute("aria-pressed", "true");

    // Click the checkerboard / empty padding around the canvas surface.
    fireEvent.pointerDown(viewport, {
      button: 0,
      pointerId: 42,
      clientX: -40,
      clientY: -30,
    });
    expect(textLayer).toHaveAttribute("aria-pressed", "false");
  });

  it("can start a crop outside the canvas and apply an edge-to-edge cut", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
    fireEvent.click(screen.getByRole("button", { name: "Crop (C)" }));
    const viewport = screen.getByLabelText("Screenshot editing canvas");
    const canvas = viewport.querySelector("canvas")!;
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

    fireEvent.pointerDown(viewport, {
      button: 0,
      pointerId: 61,
      clientX: 60,
      clientY: 40,
    });
    fireEvent.pointerMove(viewport, {
      pointerId: 61,
      clientX: 100 + 500,
      clientY: 80 + 400,
    });
    fireEvent.pointerUp(viewport, {
      button: 0,
      pointerId: 61,
      clientX: 100 + 500,
      clientY: 80 + 400,
    });

    expect(screen.getByLabelText("Crop width")).toHaveValue("500");
    expect(screen.getByLabelText("Crop height")).toHaveValue("400");
    const apply = screen.getByRole("button", { name: "Apply crop" });
    expect(apply).toHaveClass("cta-pulse");
    fireEvent.click(apply);
    expect(screen.getByLabelText("Canvas width")).toHaveValue(500);
    expect(screen.getByLabelText("Canvas height")).toHaveValue(400);
  });

  it("holds Shift during a free crop drag to keep the live aspect ratio", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
    fireEvent.click(screen.getByRole("button", { name: "Crop (C)" }));
    const canvas = screen.getByLabelText("Screenshot editing canvas").querySelector("canvas")!;
    setCanvasBounds(canvas);
    canvas.setPointerCapture = vi.fn();
    canvas.hasPointerCapture = vi.fn(() => true);
    canvas.releasePointerCapture = vi.fn();

    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 62,
      clientX: 100,
      clientY: 100,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 62,
      clientX: 300,
      clientY: 200,
    });
    expect(screen.getByLabelText("Crop width")).toHaveValue("200");
    expect(screen.getByLabelText("Crop height")).toHaveValue("100");

    fireEvent.pointerMove(canvas, {
      pointerId: 62,
      clientX: 500,
      clientY: 700,
      shiftKey: true,
    });
    const width = Number(screen.getByLabelText("Crop width").getAttribute("value"));
    const height = Number(screen.getByLabelText("Crop height").getAttribute("value"));
    expect(width / height).toBeCloseTo(2, 2);
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 62,
      clientX: 500,
      clientY: 700,
      shiftKey: true,
    });

    fireEvent.click(screen.getByRole("button", { name: "Apply crop" }));
    expect(screen.getByLabelText("Canvas width")).toHaveValue(width);
    expect(screen.getByLabelText("Canvas height")).toHaveValue(height);
  });

  it("keeps a past-edge arrow clipped until Expand canvas is clicked", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
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

    // While past the edge: faded overflow only — expand is opt-in, not on release.
    const overflow = viewport.querySelector(".screenshot-canvas-expand-overflow");
    expect(overflow).toBeTruthy();
    expect(overflow?.classList.contains("is-live")).toBe(true);
    expect(viewport.querySelector(".screenshot-canvas-expand-ghost")).toBeNull();
    expect(viewport.querySelector(".screenshot-canvas-expand-particle")).toBeNull();
    expect(screen.queryByText("Release to expand canvas")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Expand canvas" })).not.toBeInTheDocument();

    fireEvent.pointerUp(viewport, {
      button: 0,
      pointerId: 50,
      clientX: 280,
      clientY: 260,
    });

    expect(
      within(screen.getByRole("region", { name: "Layers" })).getByText("Arrow"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Canvas width")).toHaveValue(1_440);
    expect(screen.getByLabelText("Canvas height")).toHaveValue(900);
    expect(viewport.querySelector(".screenshot-canvas-expand-overflow")).toBeTruthy();

    const expand = await screen.findByRole("button", { name: "Expand canvas" });
    expect(viewport.querySelector(".screenshot-canvas-expand-particle")).toBeNull();

    fireEvent.pointerEnter(expand);
    expect(viewport.querySelector(".screenshot-canvas-expand-ghost")?.classList.contains("edge-left"))
      .toBe(true);
    expect(viewport.querySelectorAll(".screenshot-canvas-expand-particle").length).toBeGreaterThan(0);

    fireEvent.click(expand);
    expect(screen.queryByRole("button", { name: "Expand canvas" })).not.toBeInTheDocument();
    expect(viewport.querySelector(".screenshot-canvas-expand-overflow")).toBeNull();
    expect(Number(screen.getByLabelText("Canvas width").getAttribute("value"))).toBeGreaterThan(1_440);
  });

  it("still expands when a new drawing sits fully outside the canvas", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    setCanvasZoomPercent(100);
    fireEvent.click(screen.getByRole("button", { name: "Arrow (A)" }));
    const viewport = screen.getByLabelText("Screenshot editing canvas");
    const canvas = viewport.querySelector("canvas")!;
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

    fireEvent.pointerDown(viewport, {
      button: 0,
      pointerId: 51,
      clientX: 60,
      clientY: 200,
    });
    fireEvent.pointerMove(viewport, {
      pointerId: 51,
      clientX: 80,
      clientY: 220,
    });
    fireEvent.pointerUp(viewport, {
      button: 0,
      pointerId: 51,
      clientX: 80,
      clientY: 220,
    });

    expect(
      within(screen.getByRole("region", { name: "Layers" })).getByText("Arrow"),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Expand canvas" })).not.toBeInTheDocument();
    expect(Number(screen.getByLabelText("Canvas width").getAttribute("value"))).toBeGreaterThan(1_440);
  });

  it("snaps image drop guides to the closest edge without a selected layer", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const editor = screen.getByLabelText("Screenshot editing canvas").closest("main");
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
      // Top toast is the only placement label (no center-of-canvas badge).
      expect(screen.getAllByText("Place above")).toHaveLength(1);
    });
    expect(screen.queryByText("Place below")).not.toBeInTheDocument();
    expect(screen.getByText("Place above").closest(".screenshot-drop-overlay")).not.toBeNull();

    const guide = document.querySelector(".screenshot-drop-snap-guide.edge-top");
    expect(guide).not.toBeNull();
    expect(guide?.querySelector(".screenshot-drop-snap-bloom")).not.toBeNull();
    expect(guide?.querySelectorAll(".screenshot-drop-snap-particle").length).toBeGreaterThan(0);
    // Edge guides keep the snap visuals only — no second label badge on the canvas.
    expect(guide?.querySelector(":scope > span")).toBeNull();
  });

  it("emits soft stack light from the drag preview and centers its toast on the viewport", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const editor = screen.getByLabelText("Screenshot editing canvas").closest("main");
    expect(editor).toBeTruthy();
    const viewport = screen.getByLabelText("Screenshot editing canvas");
    vi.spyOn(viewport, "getBoundingClientRect").mockReturnValue({
      x: 70,
      y: 54,
      top: 54,
      left: 70,
      right: 890,
      bottom: 700,
      width: 820,
      height: 646,
      toJSON: () => ({}),
    });
    const canvas = viewport.querySelector("canvas")!;
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
    expect(screen.queryByText(/stays editable as a layer/i)).not.toBeInTheDocument();
    const toast = screen.getByText("Place on top").closest(".screenshot-drop-overlay");
    // 70 + 820 / 2 = 480: centered in the visible canvas viewport, not the window.
    expect(toast).toHaveStyle({ left: "480px", top: "72px" });

    const guide = document.querySelector(".screenshot-drop-snap-guide.edge-stack") as HTMLElement | null;
    expect(guide).not.toBeNull();
    const light = guide?.querySelector(".screenshot-drop-snap-stack-light") as HTMLElement | null;
    expect(light).not.toBeNull();
    // Elevated spill: warm pool + contact shadow + thin rim (no ray/neon layers).
    expect(light?.querySelector(".screenshot-drop-snap-stack-pool")).not.toBeNull();
    expect(light?.querySelector(".screenshot-drop-snap-stack-shadow")).not.toBeNull();
    expect(light?.querySelector(".screenshot-drop-snap-stack-rim")).not.toBeNull();
    expect(light?.querySelector(".screenshot-drop-snap-stack-atmosphere")).toBeNull();
    expect(light?.querySelectorAll(".screenshot-drop-snap-stack-rays")).toHaveLength(0);
    expect(light?.querySelectorAll(".screenshot-drop-snap-stack-ray")).toHaveLength(0);
    expect(light?.querySelector(".screenshot-drop-snap-stack-edge-glow")).toBeNull();
    // The emitter is the compact native-preview footprint, not the target layer.
    expect(Number.parseFloat(light?.style.width ?? "100")).toBeLessThan(50);
    expect(Number.parseFloat(light?.style.height ?? "100")).toBeLessThan(50);
    expect(light?.querySelector("svg, polygon")).toBeNull();
    // The real native drag preview is the only floating rectangle: no detached
    // target-wide bloom, opaque backing plate, or decorative particles.
    expect(guide?.querySelector(".screenshot-drop-snap-bloom")).toBeNull();
    expect(guide?.querySelector(".screenshot-drop-snap-stack-plate")).toBeNull();
    expect(guide?.querySelectorAll(".screenshot-drop-snap-particle")).toHaveLength(0);
    // Stack mode does not render a second on-canvas label badge.
    expect(guide?.querySelector(":scope > span")).toBeNull();
    expect(Number(light?.dataset.focusX)).toBeCloseTo(720, 0);
    expect(Number(light?.dataset.focusY)).toBeCloseTo(450, 0);
    const initialLeft = Number.parseFloat(light?.style.left ?? "0");

    // Moving the pointer moves the whole local emitter with the preview.
    const moved = createEvent.dragOver(editor!, { dataTransfer });
    Object.defineProperty(moved, "clientX", { configurable: true, value: 900 });
    Object.defineProperty(moved, "clientY", { configurable: true, value: 520 });
    fireEvent(editor!, moved);
    await waitFor(() => {
      const nextLight = document.querySelector(
        ".screenshot-drop-snap-guide.edge-stack .screenshot-drop-snap-stack-light",
      ) as HTMLElement | null;
      expect(Number(nextLight?.dataset.focusX)).toBeGreaterThan(720);
      expect(Number(nextLight?.dataset.focusY)).toBeGreaterThan(450);
      expect(Number.parseFloat(nextLight?.style.left ?? "0")).toBeGreaterThan(initialLeft);
    });
  });

  it("keeps preserve quality by default and compress shows quality presets without changing format", async () => {
    const restoreCanvas = installExportableCanvas();
    try {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    const format = screen.getByRole("combobox", { name: "Format" });
    expect(format).toHaveTextContent(".png");
    const saveQuality = screen.getByRole("combobox", { name: "Save quality" });
    expect(saveQuality).toHaveTextContent("Preserve quality");
    expect(screen.queryByRole("combobox", { name: "Compression quality" }))
      .not.toBeInTheDocument();
    expect(screen.queryByRole("spinbutton", { name: "Maximum file size" }))
      .not.toBeInTheDocument();
    expect(
      screen.getByText("Keeps original quality as PNG and replaces the original."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Compression comparison" }))
      .not.toBeInTheDocument();

    // Compress keeps PNG and shows the same Tiny–High quality ladder as JPEG.
    fireEvent.click(saveQuality);
    const compressOption = screen.getByRole("option", { name: /Compress/ });
    expect(compressOption).toHaveTextContent(
      "Smaller PNG with Tiny through High quality presets.",
    );
    expect(compressOption).not.toHaveTextContent(/compresspng/i);
    fireEvent.click(compressOption);

    await waitFor(() => {
      expect(format).toHaveTextContent(".png");
    });
    const quality = screen.getByRole("combobox", { name: "Compression quality" });
    expect(quality).toHaveTextContent("High");
    expect(screen.queryByRole("slider", { name: "PNG palette colors" }))
      .not.toBeInTheDocument();
    expect(screen.queryByRole("spinbutton", { name: "Maximum file size" }))
      .not.toBeInTheDocument();
    expect(
      screen.getByText("Compressed PNG replaces the original; turn on Save as new file to keep it."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Compare before / after" }))
      .not.toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Compression comparison" }))
      .toBeInTheDocument();

    fireEvent.click(quality);
    fireEvent.click(screen.getByRole("option", { name: /Tiny/ }));
    expect(quality).toHaveTextContent("Tiny");
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith(
        "preview_screenshot_export",
        expect.objectContaining({
          format: "png",
          qualityMode: "compress",
          jpegQuality: 55,
          pngMaxColors: 32,
        }),
      );
    });

    fireEvent.click(format);
    fireEvent.click(screen.getByRole("option", { name: "JPEG" }));
    expect(screen.getByRole("combobox", { name: "Save quality" }))
      .toHaveTextContent("Compress");
    expect(screen.getByRole("combobox", { name: "Compression quality" }))
      .toHaveTextContent("Tiny");
    fireEvent.click(screen.getByRole("combobox", { name: "Compression quality" }));
    fireEvent.click(screen.getByRole("option", { name: /Balanced/ }));
    expect(screen.getByRole("combobox", { name: "Compression quality" }))
      .toHaveTextContent("Balanced");
    // Source is still a PNG path, so a JPEG save is always a new file.
    expect(
      screen.getByText("Compressed JPEG saves as a new file and leaves the original untouched."),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
    fireEvent.click(screen.getByRole("option", { name: /Maximum file size/ }));

    expect(screen.queryByRole("combobox", { name: "Compression quality" }))
      .not.toBeInTheDocument();
    expect(screen.getByRole("spinbutton", { name: "Maximum file size" })).toHaveValue(10);
    expect(screen.getByRole("combobox", { name: "Screenshot file size unit" }))
      .toHaveTextContent("MB");
    expect(format).toHaveTextContent(".jpg");
    expect(screen.getByRole("group", { name: "Compression comparison" }))
      .toBeInTheDocument();

    // Switching format keeps the quality mode; maximum works for PNG too.
    fireEvent.click(format);
    fireEvent.click(screen.getByRole("option", { name: "PNG" }));
    expect(screen.getByRole("combobox", { name: "Save quality" }))
      .toHaveTextContent("Maximum file size");
    expect(screen.getByRole("spinbutton", { name: "Maximum file size" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Compression comparison" }))
      .toBeInTheDocument();

    fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
    fireEvent.click(screen.getByRole("option", { name: /Preserve quality/ }));
    expect(screen.queryByRole("group", { name: "Compression comparison" }))
      .not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Compression quality" }))
      .not.toBeInTheDocument();
    } finally {
      restoreCanvas();
    }
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
      await screen.findByLabelText("Canvas width");

      // Original PNG at full size with preserve quality → known capture size (250 KB).
      await waitFor(() => {
        expect(screen.getByText("≈ 250 KB")).toBeInTheDocument();
      }, { timeout: 2_000 });
      expect(screen.queryByText("−0%")).not.toBeInTheDocument();
      expect(document.querySelector(".screenshot-output-estimate-delta")).toBeNull();
      // Browser re-encode path should not run for the unedited original estimate.
      expect(toBlob).not.toHaveBeenCalled();
    } finally {
      window.Image = originalImage;
    }
  });

  it("supports explicit custom output width and height", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");

    fireEvent.click(screen.getByRole("combobox", { name: "Output size" }));
    fireEvent.click(screen.getByRole("option", { name: /Choose exact pixel dimensions/ }));
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

  it("shows a size-change percent when output size shrinks the pixel dimensions", async () => {
    const restoreCanvas = installExportableCanvas();
    Object.defineProperty(HTMLCanvasElement.prototype, "toBlob", {
      configurable: true,
      value: function toBlob(this: HTMLCanvasElement, callback: BlobCallback, type?: string) {
        const pixels = Math.max(1, this.width * this.height);
        const size = Math.max(1, Math.round(pixels * 0.12));
        const bytes = new Uint8Array(size);
        const blob = new Blob([bytes], { type: type ?? "image/png" });
        if (typeof blob.arrayBuffer !== "function") {
          Object.defineProperty(blob, "arrayBuffer", {
            value: async () => bytes.buffer,
          });
        }
        callback(blob);
      },
    });
    try {
      render(<ScreenshotEditor />);
      await screen.findByLabelText("Canvas width");
      await waitFor(() => {
        expect(screen.getByText("≈ 250 KB")).toBeInTheDocument();
      }, { timeout: 2_000 });
      expect(document.querySelector(".screenshot-output-estimate-delta")).toBeNull();

      fireEvent.click(screen.getByRole("combobox", { name: "Output size" }));
      expect(screen.getByRole("option", { name: /half the pixel width/ })).toHaveTextContent(
        "Save at half the pixel width and height.",
      );
      fireEvent.click(screen.getByRole("option", { name: /half the pixel width/ }));

      // 1440×900 → 720×450; 0.12 bytes/pixel ≈ 38.9 KB vs the 250 KB original.
      await waitFor(() => {
        expect(screen.getByText("≈ 38.9 KB")).toBeInTheDocument();
        expect(screen.getByText("−84%")).toBeInTheDocument();
      }, { timeout: 2_000 });
      expect(screen.getByText("−84%")).toHaveClass("screenshot-output-estimate-delta", "is-smaller");
      expect(screen.getByRole("combobox", { name: "Save quality" }))
        .toHaveTextContent("Preserve quality");
    } finally {
      restoreCanvas();
    }
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
      const bytes = new Uint8Array(size);
      const blob = new Blob([bytes], { type: type ?? "image/png" });
      if (typeof blob.arrayBuffer !== "function") {
        Object.defineProperty(blob, "arrayBuffer", {
          value: async () => bytes.buffer,
        });
      }
      callback(blob);
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
      measureText: (text: string) => ({
      width: Math.max(1, [...String(text ?? "")].length * 10),
    }),
      fillText: vi.fn(),
      strokeText: vi.fn(),
      translate: vi.fn(),
      transform: vi.fn(),
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
      await screen.findByLabelText("Canvas width");

      await waitFor(() => {
        expect(screen.getByTitle("Estimated export file size for the current format, quality, and output size"))
          .toHaveTextContent(/≈/);
      }, { timeout: 2_000 });

      fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
      fireEvent.click(screen.getByRole("option", { name: /Compress/ }));
      // PNG compress estimates go through Rust (quality presets map to palette size).
      expect(screen.getByRole("combobox", { name: "Format" })).toHaveTextContent(".png");
      await waitFor(() => {
        expect(screen.getByRole("combobox", { name: "Compression quality" })).toBeInTheDocument();
      });

      const estimate = () => screen.getByTitle(
        "Estimated export file size for the current format, quality, and output size",
      );
      await waitFor(() => {
        expect(vi.mocked(invoke).mock.calls.some(
          ([command]) => command === "estimate_screenshot_export",
        )).toBe(true);
        expect(estimate()).toHaveTextContent(/≈/);
      }, { timeout: 2_000 });

      // JPEG still uses the browser quality estimate path.
      fireEvent.click(screen.getByRole("combobox", { name: "Format" }));
      fireEvent.click(screen.getByRole("option", { name: "JPEG" }));
      await waitFor(() => {
        expect(screen.getByRole("combobox", { name: "Format" })).toHaveTextContent(".jpg");
      });
      fireEvent.click(screen.getByRole("combobox", { name: "Compression quality" }));
      fireEvent.click(screen.getByRole("option", { name: /High/ }));
      await waitFor(() => {
        expect(toBlob.mock.calls.some((call) => call[1] === "image/jpeg")).toBe(true);
        expect(estimate()).toHaveTextContent(/≈/);
      }, { timeout: 2_000 });
      const highEstimate = estimate().textContent;

      fireEvent.click(screen.getByRole("combobox", { name: "Compression quality" }));
      fireEvent.click(screen.getByRole("option", { name: /Tiny/ }));

      await waitFor(() => {
        expect(estimate()).toHaveTextContent(/≈/);
        // Lower JPEG quality should yield a smaller estimated encode.
        expect(estimate().textContent).not.toBe(highEstimate);
      }, { timeout: 2_000 });
    } finally {
      window.Image = originalImage;
    }
  });

  it("compares Est. size to the original image when switching Tiny back to High", async () => {
    const restoreCanvas = installExportableCanvas();
    const compact: CaptureArtifact = { ...artifact, size_bytes: 188_000 };
    const sourcePngBytes = 1_100_000;
    const tinyBytes = 188_000;
    const highBytes = 301_000;

    Object.defineProperty(HTMLCanvasElement.prototype, "toBlob", {
      configurable: true,
      value: (callback: BlobCallback, type?: string) => {
        const size = type === "image/png" ? sourcePngBytes : 64;
        const bytes = new Uint8Array(size);
        const blob = new Blob([bytes], { type: type ?? "image/png" });
        if (typeof blob.arrayBuffer !== "function") {
          Object.defineProperty(blob, "arrayBuffer", {
            value: async () => bytes.buffer,
          });
        }
        callback(blob);
      },
    });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_artifact") return compact;
      if (command === "estimate_screenshot_export" || command === "preview_screenshot_export") {
        const colors = Number((args as { pngMaxColors?: number } | undefined)?.pngMaxColors ?? 256);
        const sizeBytes = colors <= 32 ? tinyBytes : highBytes;
        if (command === "estimate_screenshot_export") return sizeBytes;
        return {
          bytes: [1, 2, 3],
          sizeBytes,
          format: "png",
        };
      }
      const draft = draftCommandResult(String(command));
      if (draft !== undefined || String(command).includes("screenshot_editor_draft")) {
        return draft;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    try {
      render(<ScreenshotEditor />);
      await screen.findByLabelText("Canvas width");

      fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
      fireEvent.click(screen.getByRole("option", { name: /Compress/ }));

      const estimate = () => screen.getByTitle(
        "Estimated export file size for the current format, quality, and output size",
      );

      // Default compress preset is High. 301 KB vs the 1.1 MB Before image is −73%,
      // not +60% versus the compact 188 KB file (or a previous Tiny estimate).
      await waitFor(() => {
        expect(estimate()).toHaveTextContent("≈ 301 KB");
        expect(screen.getByText("−73%")).toBeInTheDocument();
      }, { timeout: 3_000 });
      expect(screen.getByText("−73%")).toHaveClass("screenshot-output-estimate-delta", "is-smaller");
      expect(screen.queryByText("+60%")).not.toBeInTheDocument();

      fireEvent.click(screen.getByRole("combobox", { name: "Compression quality" }));
      fireEvent.click(screen.getByRole("option", { name: /Tiny/ }));
      await waitFor(() => {
        expect(estimate()).toHaveTextContent("≈ 188 KB");
        expect(screen.getByText("−83%")).toBeInTheDocument();
      }, { timeout: 3_000 });

      fireEvent.click(screen.getByRole("combobox", { name: "Compression quality" }));
      fireEvent.click(screen.getByRole("option", { name: /High/ }));
      await waitFor(() => {
        expect(estimate()).toHaveTextContent("≈ 301 KB");
        expect(screen.getByText("−73%")).toBeInTheDocument();
      }, { timeout: 3_000 });
      expect(screen.queryByText("+60%")).not.toBeInTheDocument();
    } finally {
      restoreCanvas();
    }
  });

  it("uses the current estimate baseline when a later compression preview fails", async () => {
    const restoreCanvas = installExportableCanvas();
    const compact: CaptureArtifact = { ...artifact, size_bytes: 188_000 };
    const fullPngBytes = 100_000;
    const halfPngBytes = 25_000;
    let failPreview = false;

    Object.defineProperty(HTMLCanvasElement.prototype, "toBlob", {
      configurable: true,
      value: function toBlob(this: HTMLCanvasElement, callback: BlobCallback, type?: string) {
        const size = type === "image/png"
          ? (this.width >= 1_000 ? fullPngBytes : halfPngBytes)
          : 64;
        const bytes = new Uint8Array(size);
        const blob = new Blob([bytes], { type: type ?? "image/png" });
        if (typeof blob.arrayBuffer !== "function") {
          Object.defineProperty(blob, "arrayBuffer", {
            value: async () => bytes.buffer,
          });
        }
        callback(blob);
      },
    });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_artifact") return compact;
      if (command === "preview_screenshot_export") {
        if (failPreview) throw new Error("preview encode failed");
        const imagePng = (args as { imagePng?: number[] } | undefined)?.imagePng;
        const sizeBytes = Math.round((imagePng?.length ?? fullPngBytes) / 2);
        return { bytes: [1, 2, 3], sizeBytes, format: "png" };
      }
      if (command === "estimate_screenshot_export") {
        const imagePng = (args as { imagePng?: number[] } | undefined)?.imagePng;
        return Math.round((imagePng?.length ?? fullPngBytes) / 2);
      }
      const draft = draftCommandResult(String(command));
      if (draft !== undefined || String(command).includes("screenshot_editor_draft")) {
        return draft;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    try {
      render(<ScreenshotEditor />);
      await screen.findByLabelText("Canvas width");
      fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
      fireEvent.click(screen.getByRole("option", { name: /Compress/ }));

      const estimate = () => screen.getByTitle(
        "Estimated export file size for the current format, quality, and output size",
      );
      await waitFor(() => {
        expect(estimate()).toHaveTextContent("≈ 50.0 KB");
        expect(screen.getByText("−50%")).toBeInTheDocument();
      }, { timeout: 3_000 });

      failPreview = true;
      fireEvent.click(screen.getByRole("combobox", { name: "Output size" }));
      fireEvent.click(screen.getByRole("option", { name: /half the pixel width/ }));

      // Half-size source is 25 KB; estimate 12.5 KB is −50% versus that canvas,
      // not −88% versus the stale full-size Before bytes.
      await waitFor(() => {
        expect(estimate()).toHaveTextContent("≈ 12.5 KB");
        expect(screen.getByText("−50%")).toBeInTheDocument();
      }, { timeout: 3_000 });
      expect(screen.queryByText("−88%")).not.toBeInTheDocument();
    } finally {
      restoreCanvas();
    }
  });

  it("defaults path-less captures to a plain filename without an -edited suffix", async () => {
    const pathless: CaptureArtifact = {
      ...artifact,
      path: null,
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifact") return pathless;
      if (command === "default_screenshot_edit_path") {
        return "/Users/example/Captures/Captures_2026-08-08_12-00-00_000.png";
      }
      const draft = draftCommandResult(String(command));
      if (draft !== undefined || String(command).includes("screenshot_editor_draft")) {
        return draft;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<ScreenshotEditor />);
    expect(await screen.findByRole("textbox", { name: "Saved filename" }))
      .toHaveValue("Captures_2026-08-08_12-00-00_000");
    expect(screen.getByLabelText("Save location"))
      .toHaveTextContent("/Users/example/Captures");
    // No permanent original yet — Save as new file is hidden until the first save.
    expect(screen.queryByRole("checkbox", { name: "Save as new file" }))
      .not.toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("default_screenshot_edit_path", {
      artifactId: pathless.id,
      format: "png",
    });
  });

  it("defaults export format from screenshot preferences", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifact") return artifact;
      if (command === "get_settings") return { screenshot_format: "jpeg" };
      const draft = draftCommandResult(String(command));
      if (draft !== undefined || String(command).includes("screenshot_editor_draft")) {
        return draft;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<ScreenshotEditor />);
    await screen.findByLabelText("Canvas width");
    expect(screen.getByRole("combobox", { name: "Format" })).toHaveTextContent(".jpg");
  });

  it("uses the preferred format when suggesting a first-save path", async () => {
    const pathless: CaptureArtifact = {
      ...artifact,
      path: null,
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifact") return pathless;
      if (command === "get_settings") return { screenshot_format: "webp" };
      if (command === "default_screenshot_edit_path") {
        return "/Users/example/Captures/Captures_2026-08-08_12-00-00_000.webp";
      }
      const draft = draftCommandResult(String(command));
      if (draft !== undefined || String(command).includes("screenshot_editor_draft")) {
        return draft;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<ScreenshotEditor />);
    expect(await screen.findByRole("textbox", { name: "Saved filename" }))
      .toHaveValue("Captures_2026-08-08_12-00-00_000");
    expect(screen.getByRole("combobox", { name: "Format" })).toHaveTextContent(".webp");
    expect(invoke).toHaveBeenCalledWith("default_screenshot_edit_path", {
      artifactId: pathless.id,
      format: "webp",
    });
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
      if (command === "preview_screenshot_export") {
        return {
          bytes: [1, 2, 3],
          sizeBytes: 12_000,
          format: "png",
        };
      }
      if (command === "estimate_screenshot_export") return 12_000;
      if (command === "reveal_artifact") return undefined;
      const draft = draftCommandResult(String(command));
      if (draft !== undefined || String(command).includes("screenshot_editor_draft")) {
        return draft;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    try {
      render(<ScreenshotEditor />);
      expect(await screen.findByRole("textbox", { name: "Saved filename" }))
        .toHaveValue("capture");
      expect(screen.getByLabelText("Save location"))
        .toHaveTextContent("/Users/example/Captures");
      expect(screen.getByRole("checkbox", { name: "Save as new file" })).not.toBeChecked();

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
      fireEvent.click(screen.getByRole("combobox", { name: "Save quality" }));
      fireEvent.click(screen.getByRole("option", { name: /Maximum file size/ }));
      // Maximum size keeps the selected PNG format.
      expect(screen.getByRole("combobox", { name: "Format" })).toHaveTextContent(".png");
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
      // so later Save overwrites and Save as new file creates another file.
      await waitFor(() => {
        expect(screen.getByRole("textbox", { name: "Saved filename" }))
          .toHaveValue("edited-photo");
        expect(screen.getByLabelText("Save location"))
          .toHaveTextContent("/Users/example/Pictures");
        expect(screen.getByRole("checkbox", { name: "Save as new file" })).not.toBeChecked();
      });
      fireEvent.click(screen.getByRole("checkbox", { name: "Save as new file" }));
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
    await screen.findByLabelText("Canvas width");

    expect(screen.getByRole("button", { name: "Copy image" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Copy image" })).toHaveAttribute(
      "title",
      "Copy the edited image to the clipboard",
    );
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    expect(screen.getByRole("checkbox", { name: "Save as new file" })).not.toBeChecked();
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
    expect(screen.queryByRole("checkbox", { name: "Save as new file" }))
      .not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows a success state for copy without replacing the export hint", async () => {
    const restoreCanvas = installExportableCanvas();
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifact") return artifact;
      if (command === "copy_screenshot_edit") return undefined;
      const draft = draftCommandResult(String(command));
      if (draft !== undefined || String(command).includes("screenshot_editor_draft")) {
        return draft;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    try {
      render(<ScreenshotEditor />);
      await screen.findByLabelText("Canvas width");

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
