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
      "Remove bg (B)",
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

  it("pans the canvas with Command/Ctrl-drag from the canvas surface", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

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
    await screen.findByLabelText("Width");

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
    expect(screen.getByText(/Double-click the path to add a point/)).toBeInTheDocument();

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

  it("adds and removes arrow curve points with double-click", async () => {
    render(<ScreenshotEditor />);
    await screen.findAllByText("1440 × 900");

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
    fireEvent.click(within(layers).getByRole("button", { name: /ArrowShape/ }));
    expect(screen.getByText(/\(0\/4\)/)).toBeInTheDocument();

    // Create a first bend so the path has room for another point.
    fireEvent.pointerDown(canvas, {
      button: 0,
      pointerId: 41,
      clientX: 250,
      clientY: 200,
    });
    fireEvent.pointerMove(canvas, {
      pointerId: 41,
      clientX: 250,
      clientY: 280,
    });
    fireEvent.pointerUp(canvas, {
      button: 0,
      pointerId: 41,
      clientX: 250,
      clientY: 280,
    });
    expect(screen.getByText(/\(1\/4\)/)).toBeInTheDocument();

    // Double-click farther along the shaft to insert a second control.
    fireEvent.doubleClick(canvas, {
      button: 0,
      clientX: 340,
      clientY: 230,
    });
    expect(screen.getByText(/\(2\/4\)/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Straighten arrow" })).toBeInTheDocument();

    // Double-click the first control handle to remove it.
    fireEvent.doubleClick(canvas, {
      button: 0,
      clientX: 250,
      clientY: 280,
    });
    expect(screen.getByText(/\(1\/4\)/)).toBeInTheDocument();
    expect(screen.getByRole("slider", { name: "Curve" })).toBeInTheDocument();
  });

  it("curves lines with the same handles, double-click points, and hover tip", async () => {
    render(<ScreenshotEditor />);
    await screen.findAllByText("1440 × 900");

    fireEvent.change(screen.getByRole("combobox", { name: "Canvas zoom" }), {
      target: { value: "100" },
    });
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
    expect(screen.getByText(/Double-click the path to add a point/)).toBeInTheDocument();

    // Mid handle bend (chord length 300 → 150px lateral = 50% curve).
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
    expect(screen.getByRole("slider", { name: "Curve" })).toHaveValue("50");

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
    expect(screen.getByText(/\(2\/4\)/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Straighten line" })).toBeInTheDocument();
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
    const layerTools = screen.getByRole("toolbar", { name: "Layer actions" });
    expect(within(layerTools).getByRole("button", { name: "Duplicate" })).toBeEnabled();
    expect(within(layerTools).getByRole("button", { name: "Delete" })).toBeEnabled();
    // Single-layer document: nothing to merge into, and flatten only bakes when
    // a solid canvas background remains (it does by default).
    expect(within(layerTools).getByRole("button", { name: "Merge down" })).toBeDisabled();
    expect(within(layerTools).getByRole("button", { name: "Merge visible" })).toBeDisabled();
    expect(within(layerTools).getByRole("button", { name: "Flatten image" })).toBeEnabled();
    expect(
      within(layers).getByRole("button", { name: "Lock Original screenshot" }),
    ).toHaveAttribute("aria-pressed", "false");
    expect(within(layers).getByText("Background")).toBeInTheDocument();
  });

  it("exposes remove-background modes and wand controls", async () => {
    render(<ScreenshotEditor />);
    await screen.findByLabelText("Width");

    fireEvent.click(screen.getByRole("button", { name: "Remove bg (B)" }));
    expect(screen.getByRole("button", { name: "Wand" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText("Color tolerance")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Contiguous only" })).toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: "Erase" }));
    expect(screen.getByRole("button", { name: "Erase" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText("Brush size")).toBeInTheDocument();
    expect(screen.queryByLabelText("Color tolerance")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Restore" }));
    expect(screen.getByRole("button", { name: "Restore" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText("Brush size")).toBeInTheDocument();
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

  it("previews margins that Trim edges would remove while hovering the control", async () => {
    render(<ScreenshotEditor />);
    const widthInput = await screen.findByLabelText("Width");
    const canvasToolbar = screen.getByRole("group", { name: "Canvas" });
    const canvas = screen.getByLabelText("Screenshot editing canvas");

    fireEvent.change(widthInput, { target: { value: "1600" } });
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

    // While past the edge: faded overflow paint + quiet ghost with stronger left edge.
    const overflow = viewport.querySelector(".screenshot-canvas-expand-overflow");
    const ghost = viewport.querySelector(".screenshot-canvas-expand-ghost");
    expect(overflow).toBeTruthy();
    expect(ghost).toBeTruthy();
    expect(ghost?.classList.contains("edge-left")).toBe(true);
    expect(screen.getByText("Release to expand canvas")).toBeInTheDocument();

    fireEvent.pointerUp(viewport, {
      button: 0,
      pointerId: 50,
      clientX: 280,
      clientY: 260,
    });

    expect(
      within(screen.getByRole("region", { name: "Layers" })).getByText("Arrow"),
    ).toBeInTheDocument();
    // Preview chrome clears after release.
    expect(viewport.querySelector(".screenshot-canvas-expand-overflow")).toBeNull();
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
    await screen.findByLabelText("Width");

    const editor = screen.getByText("Screenshot editor").closest("main");
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
      throw new Error(`unexpected command: ${command}`);
    });

    render(<ScreenshotEditor />);
    expect(await screen.findByRole("textbox", { name: "Saved filename" }))
      .toHaveValue("Captures_2026-08-08_12-00-00_000");
    expect(screen.getByLabelText("Save location"))
      .toHaveTextContent("/Users/example/Captures");
    // No permanent original yet — Make a copy is hidden until the first save.
    expect(screen.queryByRole("checkbox", { name: "Make a copy" }))
      .not.toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("default_screenshot_edit_path", {
      artifactId: pathless.id,
      format: "png",
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
