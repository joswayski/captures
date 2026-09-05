import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

import { Thumbnail } from "./App";
import type { CaptureArtifact } from "./types";
import {
  THUMBNAIL_CARD_SLOT_PX,
  THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS,
  THUMBNAIL_STACK_EXPAND_COLLAPSE_MS,
  thumbnailStackFanCollapseMs,
  thumbnailStackNewestScrollTop,
  thumbnailStackSkewCssVars,
  thumbnailStackPeekJitterPx,
} from "./lib/thumbnailLayout";
import {
  THUMBNAIL_HOVER_STALE_ATTRIBUTE,
  THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE,
  THUMBNAIL_SUPPRESS_CARD_HOVER_ATTRIBUTE,
} from "./lib/thumbnailHover";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}));

const artifact: CaptureArtifact = {
  id: "capture-1",
  path: null,
  preview_url: "captures-capture://artifact/capture-1",
  full_url: "captures-capture://artifact-full/capture-1",
  width: 1_440,
  height: 900,
  size_bytes: 250_000,
  created_at: "2026-07-19T18:00:00Z",
  mode: "region",
  history_saved: true,
  clipboard_copy_status: "copied",
};

describe("Thumbnail", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: artifact.id };
      }
      if (command === "get_thumbnail_pointer_position") {
        return new Promise(() => undefined);
      }
      return undefined;
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    document.documentElement.classList.remove("thumbnail-native-tracking");
    document.documentElement.removeAttribute("data-thumbnail-cursor");
    document.documentElement.style.cursor = "";
    Reflect.deleteProperty(document, "elementFromPoint");
    vi.clearAllMocks();
    document.documentElement.style.removeProperty("--thumbnail-stack-drag-x");
    document.documentElement.style.removeProperty("--thumbnail-stack-drag-y");
  });

  it("preserves the native hover presentation while focus moves to a viewer", async () => {
    render(<Thumbnail />);

    const card = await screen.findByRole("article");
    document.documentElement.classList.add("thumbnail-native-tracking");
    card.setAttribute("data-thumbnail-native-active", "true");

    window.dispatchEvent(new Event("focus"));

    expect(document.documentElement).toHaveClass("thumbnail-native-tracking");
    expect(card).toHaveAttribute("data-thumbnail-native-active", "true");
  });

  it("reasserts the interactive cursor across button and focus handoffs", async () => {
    let pointerReady = false;
    const editButtonRef = { current: null as HTMLElement | null };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: artifact.id };
      }
      if (command === "get_thumbnail_pointer_position") {
        return pointerReady
          ? { x: 40, y: 20, inside: true }
          : new Promise(() => undefined);
      }
      return undefined;
    });
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => editButtonRef.current),
    });

    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const edit = within(card).getByRole("button", { name: "Edit" });
    editButtonRef.current = edit;
    vi.spyOn(edit, "getBoundingClientRect").mockReturnValue({
      x: 20,
      y: 10,
      top: 10,
      left: 20,
      right: 80,
      bottom: 50,
      width: 60,
      height: 40,
      toJSON: () => ({}),
    });
    pointerReady = true;
    window.dispatchEvent(new Event("captures-thumbnail-ready"));

    await waitFor(() => {
      expect(edit).toHaveAttribute("data-native-pointer-hover", "true");
    });
    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "set_thumbnail_cursor",
        { kind: "pointer" },
      );
    });
    vi.mocked(invoke).mockClear();

    // Clicks reset the AppKit arrow; pointerdown must reassert without waiting
    // for a poll. Follow-up delays cover WebKit's post-click arrow and the
    // Edit→editor key-window handoff.
    edit.dispatchEvent(new PointerEvent("pointerdown", {
      bubbles: true,
      cancelable: true,
      button: 0,
      pointerType: "mouse",
    }));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "reassert_thumbnail_cursor",
        { kind: "pointer" },
      );
    });

    vi.mocked(invoke).mockClear();
    window.dispatchEvent(new Event("blur"));

    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "reassert_thumbnail_cursor",
      { kind: "pointer" },
    );

    // Delayed handoff ticks must keep reasserting while the editor steals focus.
    await waitFor(() => {
      expect(
        vi.mocked(invoke).mock.calls.filter(
          ([command]) => command === "reassert_thumbnail_cursor",
        ).length,
      ).toBeGreaterThan(1);
    });
  });

  it("applies the grab cursor on first hover over the preview image", async () => {
    let pointerReady = false;
    const imageRef = { current: null as HTMLElement | null };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: artifact.id };
      }
      if (command === "get_thumbnail_pointer_position") {
        return pointerReady
          ? { x: 40, y: 80, inside: true }
          : new Promise(() => undefined);
      }
      return undefined;
    });
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => imageRef.current),
    });

    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const image = within(card).getByRole("img", { name: "Screenshot preview" });
    imageRef.current = image;
    pointerReady = true;
    window.dispatchEvent(new Event("captures-thumbnail-ready"));

    await waitFor(() => {
      expect(card).toHaveAttribute("data-thumbnail-native-active", "true");
    });
    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "set_thumbnail_cursor",
        { kind: "grab" },
      );
    });
    expect(document.documentElement).toHaveAttribute(
      "data-thumbnail-cursor",
      "grab",
    );
    expect(document.documentElement.style.cursor).toBe("grab");

    // Stationary first entry must also schedule a handoff reassert so AppKit
    // open-hand survives makeKey without requiring a detour over a button.
    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "reassert_thumbnail_cursor",
        { kind: "grab" },
      );
    });
  });

  it("preserves native hover when pointer polling is briefly unavailable", async () => {
    let pointerPolls = 0;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: artifact.id };
      }
      if (command === "get_thumbnail_pointer_position") {
        pointerPolls += 1;
        return null;
      }
      return undefined;
    });

    render(<Thumbnail />);

    const card = await screen.findByRole("article");
    document.documentElement.classList.add("thumbnail-native-tracking");
    card.setAttribute("data-thumbnail-native-active", "true");
    const pollsBeforeFocus = pointerPolls;

    window.dispatchEvent(new Event("focus"));
    await waitFor(() => expect(pointerPolls).toBeGreaterThan(pollsBeforeFocus));

    expect(document.documentElement).toHaveClass("thumbnail-native-tracking");
    expect(card).toHaveAttribute("data-thumbnail-native-active", "true");
  });

  it("re-arms preview hit testing after the page becomes visible again", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: artifact.id };
      }
      if (command === "get_thumbnail_pointer_position") return null;
      return undefined;
    });

    render(<Thumbnail />);
    await screen.findByRole("article");

    document.documentElement.classList.add("thumbnail-native-tracking");
    Object.defineProperty(document, "hidden", {
      configurable: true,
      get: () => false,
    });

    document.dispatchEvent(new Event("visibilitychange"));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("refresh_thumbnail_interactivity");
    });
    expect(document.documentElement).not.toHaveClass("thumbnail-native-tracking");
  });

  it("applies grab and pointer cursors from DOM hover without waiting for a click", async () => {
    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const image = within(card).getByRole("img", { name: "Screenshot preview" });
    const minimize = screen.getByRole("button", { name: "Minimize previews" });
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => image),
    });

    fireEvent.pointerMove(image, { clientX: 80, clientY: 90, pointerType: "mouse" });

    await waitFor(() => {
      expect(document.documentElement).toHaveAttribute("data-thumbnail-cursor", "grab");
    });
    expect(document.documentElement.style.cursor).toBe("grab");
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("set_thumbnail_cursor", { kind: "grab" });

    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => minimize),
    });
    fireEvent.pointerMove(minimize, { clientX: 40, clientY: 20, pointerType: "mouse" });

    await waitFor(() => {
      expect(document.documentElement).toHaveAttribute("data-thumbnail-cursor", "pointer");
    });
    expect(document.documentElement.style.cursor).toBe("pointer");
    expect(minimize).toHaveAttribute("data-native-pointer-hover", "true");
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("set_thumbnail_cursor", { kind: "pointer" });
  });

  it("does not click-through the window from DOM hover over empty preview space", async () => {
    render(<Thumbnail />);
    const stack = (await screen.findByRole("article")).closest(".thumbnail-stack")!;
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => stack),
    });
    vi.mocked(invoke).mockClear();

    fireEvent.pointerMove(stack, { clientX: 8, clientY: 8, pointerType: "mouse" });
    window.dispatchEvent(new PointerEvent("pointerleave", { bubbles: true, pointerType: "mouse" }));

    expect(
      vi.mocked(invoke).mock.calls.filter(
        ([command, payload]) => command === "set_thumbnail_ignore_cursor_events"
          && (payload as { ignore?: boolean } | undefined)?.ignore === true,
      ),
    ).toHaveLength(0);
  });

  it("resumes WebView polling after a native show without recursively refreshing the window", async () => {
    render(<Thumbnail />);
    await screen.findByRole("article");
    vi.mocked(invoke).mockClear();

    fireEvent(window, new Event("captures-thumbnail-resumed"));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "set_thumbnail_ignore_cursor_events",
        { ignore: false },
      );
    });
    expect(vi.mocked(invoke).mock.calls.some(([command]) => (
      command === "refresh_thumbnail_interactivity"
    ))).toBe(false);
  });

  it("rejects inbound drags so a dropped screenshot cannot replace the preview UI", async () => {
    render(<Thumbnail />);
    await screen.findByRole("article");

    for (const type of ["dragover", "drop"]) {
      const dataTransfer = { dropEffect: "copy" } as DataTransfer;
      const event = new Event(type, { bubbles: true, cancelable: true }) as DragEvent;
      Object.defineProperty(event, "dataTransfer", { value: dataTransfer });

      document.body.dispatchEvent(event);

      expect(event).toHaveProperty("defaultPrevented", true);
      expect(dataTransfer.dropEffect).toBe("none");
    }
  });

  it("shows scroll cues for clipped previews and moves one card per click", async () => {
    render(<Thumbnail />);
    await screen.findByRole("article");
    const stack = document.querySelector<HTMLElement>(".thumbnail-stack")!;
    // Layout height for one card is 240px (28 top + 160 card + 52 gutter).
    Object.defineProperties(stack, {
      clientHeight: { configurable: true, value: 100 },
    });
    // maxScroll = 140. Pin near bottom.
    stack.scrollTop = 140;

    fireEvent.scroll(stack);

    const older = await screen.findByRole("button", { name: "Show older captures" });
    expect(screen.queryByRole("button", { name: "Show newer captures" })).toBeNull();

    const frames: FrameRequestCallback[] = [];
    const raf = vi.spyOn(window, "requestAnimationFrame").mockImplementation((cb) => {
      frames.push(cb);
      return frames.length;
    });
    vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => undefined);
    const now = vi.spyOn(performance, "now").mockReturnValue(0);

    fireEvent.click(older);

    expect(frames).toHaveLength(1);
    now.mockReturnValue(380);
    frames[0]?.(380);
    // One slot up from 140 → 0 (clamped).
    expect(stack.scrollTop).toBe(0);

    raf.mockRestore();
    now.mockRestore();

    stack.scrollTop = 50;
    fireEvent.scroll(stack);
    expect(await screen.findByRole("button", { name: "Show newer captures" }))
      .toBeInTheDocument();
  });

  it("keeps other previews interactive while a deleted slot passes clicks through", async () => {
    let pointerReady = false;
    let pointerTarget: Element | null = null;
    const secondArtifact = {
      ...artifact,
      id: "capture-2",
      preview_url: "captures-capture://artifact/capture-2",
      full_url: "captures-capture://artifact-full/capture-2",
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact, secondArtifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: secondArtifact.id };
      }
      if (command === "get_thumbnail_pointer_position") {
        return pointerReady ? { x: 40, y: 40, inside: true } : null;
      }
      return undefined;
    });
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => pointerTarget),
    });

    render(<Thumbnail />);
    const cards = await screen.findAllByRole("article");
    const minimizePreviews = screen.getByRole("button", { name: "Minimize previews" });
    const firstDelete = within(cards[0]).getByRole("button", { name: "Delete" });
    const secondDelete = within(cards[1]).getByRole("button", { name: "Delete" });
    pointerReady = true;
    pointerTarget = firstDelete;

    fireEvent.click(firstDelete);
    expect(minimizePreviews).toBeEnabled();

    await waitFor(() => {
      const ignoreCalls = vi.mocked(invoke).mock.calls
        .filter(([command]) => command === "set_thumbnail_ignore_cursor_events");
      expect(ignoreCalls.at(-1)?.[1]).toEqual({ ignore: true });
    });

    pointerTarget = secondDelete;
    window.dispatchEvent(new Event("captures-thumbnail-layout-changed"));
    await waitFor(() => {
      const ignoreCalls = vi.mocked(invoke).mock.calls
        .filter(([command]) => command === "set_thumbnail_ignore_cursor_events");
      expect(ignoreCalls.at(-1)?.[1]).toEqual({ ignore: false });
    });

    fireEvent.click(secondDelete);
    expect(cards[0]).toHaveClass("thumbnail-exit-delete");
    expect(cards[1]).toHaveClass("thumbnail-exit-delete");
    expect(screen.getByRole("button", { name: "Minimize previews" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Minimize previews" }).closest(".thumbnail-stack-toolbar"))
      .toHaveClass("thumbnail-stack-toolbar-exiting");
  });

  it("keeps a slid preview in place when it is deleted before the hole below is removed", async () => {
    const secondArtifact = {
      ...artifact,
      id: "capture-2",
      preview_url: "captures-capture://artifact/capture-2",
      full_url: "captures-capture://artifact-full/capture-2",
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact, secondArtifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: secondArtifact.id };
      }
      if (command === "get_thumbnail_pointer_position") return null;
      return undefined;
    });

    render(<Thumbnail />);
    const cards = await screen.findAllByRole("article");
    expect(cards).toHaveLength(2);

    vi.useFakeTimers();
    try {
      fireEvent.click(within(cards[1]).getByRole("button", { name: "Delete" }));
      expect(cards[1]).toHaveClass("thumbnail-exiting");

      await act(async () => {
        await vi.advanceTimersByTimeAsync(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      });
      expect(cards[0]).toHaveClass("thumbnail-stack-shifting");
      expect(cards[0].style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );

      fireEvent.click(within(cards[0]).getByRole("button", { name: "Delete" }));
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });

      expect(cards[0]).toHaveClass("thumbnail-exit-delete");
      expect(cards[0]).toHaveClass("thumbnail-exiting");
      expect(cards[0]).toHaveClass("thumbnail-stack-shifting");
      expect(cards[0].style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );
      expect(cards[0].style.translate).toBe(`0 ${THUMBNAIL_CARD_SLOT_PX}px`);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps collapsed previews aligned while a delete slot is still held", async () => {
    const secondArtifact = {
      ...artifact,
      id: "capture-2",
      preview_url: "captures-capture://artifact/capture-2",
      full_url: "captures-capture://artifact-full/capture-2",
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact, secondArtifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: secondArtifact.id };
      }
      if (command === "get_thumbnail_pointer_position") return null;
      return undefined;
    });

    render(<Thumbnail />);
    const cards = await screen.findAllByRole("article");
    const stack = cards[0].closest(".thumbnail-stack")!;

    vi.useFakeTimers();
    try {
      fireEvent.click(within(cards[1]).getByRole("button", { name: "Delete" }));
      expect(cards[1]).toHaveClass("thumbnail-exiting");

      fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(32);
        await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
      });
      expect(stack).toHaveClass("thumbnail-stack-compact");
      expect(stack).toHaveClass("thumbnail-stack-minimized");

      await act(async () => {
        await vi.advanceTimersByTimeAsync(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      });
      expect(cards[0]).not.toHaveClass("thumbnail-stack-shifting");
      expect(cards[0].style.getPropertyValue("--thumbnail-stack-shift")).toBe("");
      expect(cards[0].style.translate).toBe("");
      expect(cards[0].style.getPropertyValue("--thumbnail-stack-shift-slots")).toBe("");

      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "Expand 2 previews" }));
        await Promise.resolve();
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
      });
      expect(stack).not.toHaveClass("thumbnail-stack-compact");
      expect(cards[0]).toHaveClass("thumbnail-stack-shifting");
      expect(cards[0].style.translate).toBe(`0 ${THUMBNAIL_CARD_SLOT_PX}px`);
      expect(cards[0].style.getPropertyValue("--thumbnail-stack-shift-slots")).toBe("");
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps a settled survivor in place when collapsing mid-delete", async () => {
    const secondArtifact = {
      ...artifact,
      id: "capture-2",
      preview_url: "captures-capture://artifact/capture-2",
      full_url: "captures-capture://artifact-full/capture-2",
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact, secondArtifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: secondArtifact.id };
      }
      if (command === "get_thumbnail_pointer_position") return null;
      return undefined;
    });

    render(<Thumbnail />);
    const cards = await screen.findAllByRole("article");
    const stack = cards[0].closest(".thumbnail-stack")!;

    vi.useFakeTimers();
    try {
      fireEvent.click(within(cards[1]).getByRole("button", { name: "Delete" }));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      });
      expect(cards[0].style.translate).toBe(`0 ${THUMBNAIL_CARD_SLOT_PX}px`);

      fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(stack).toHaveClass("thumbnail-stack-compact");
      expect(cards[0].style.translate).toBe("");
      expect(cards[0].style.getPropertyValue("--thumbnail-stack-shift-slots")).toBe("1");
      expect(cards[0]).toHaveStyle({
        "--thumbnail-stack-base-depth": "1",
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not let an upper preview slide into a deleting neighbor during a stacked settle", async () => {
    const captures = [1, 2, 3, 4].map((n) => ({
      ...artifact,
      id: `capture-${n}`,
      preview_url: `captures-capture://artifact/capture-${n}`,
      full_url: `captures-capture://artifact-full/capture-${n}`,
    }));
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return captures;
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: captures[3].id };
      }
      if (command === "get_thumbnail_pointer_position") return null;
      return undefined;
    });

    render(<Thumbnail />);
    const cards = await screen.findAllByRole("article");
    expect(cards).toHaveLength(4);

    vi.useFakeTimers();
    try {
      fireEvent.click(within(cards[2]).getByRole("button", { name: "Delete" }));
      expect(cards[2]).toHaveClass("thumbnail-exiting");

      await act(async () => {
        await vi.advanceTimersByTimeAsync(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      });
      expect(cards[0]).toHaveClass("thumbnail-stack-shifting");
      expect(cards[1]).toHaveClass("thumbnail-stack-shifting");

      fireEvent.click(within(cards[1]).getByRole("button", { name: "Delete" }));
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });

      expect(cards[1]).toHaveClass("thumbnail-exiting");
      expect(cards[0].style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );
      expect(cards[1].style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );

      await act(async () => {
        await vi.advanceTimersByTimeAsync(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      });
      expect(cards[0].style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${THUMBNAIL_CARD_SLOT_PX}px`,
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("slides later previews up into a deleted slot when the stack is top-anchored", async () => {
    const captures = [1, 2, 3].map((n) => ({
      ...artifact,
      id: `capture-${n}`,
      preview_url: `captures-capture://artifact/capture-${n}`,
      full_url: `captures-capture://artifact-full/capture-${n}`,
    }));
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return captures;
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: captures[2].id };
      }
      if (command === "get_settings") {
        return { mini_preview_placement: "top_left" };
      }
      if (command === "get_thumbnail_pointer_position") return null;
      return undefined;
    });

    render(<Thumbnail />);
    const cards = await screen.findAllByRole("article");
    expect(cards).toHaveLength(3);
    const stack = cards[0].closest(".thumbnail-stack")!;
    await waitFor(() => {
      expect(stack).toHaveClass("thumbnail-stack-anchor-top");
    });
    // Top-anchored lists render newest-first: capture-3, capture-2, capture-1.
    expect(cards[0]).toHaveAttribute("data-thumbnail-id", "capture-3");
    expect(cards[1]).toHaveAttribute("data-thumbnail-id", "capture-2");
    expect(cards[2]).toHaveAttribute("data-thumbnail-id", "capture-1");

    vi.useFakeTimers();
    try {
      fireEvent.click(within(cards[1]).getByRole("button", { name: "Delete" }));
      expect(cards[1]).toHaveClass("thumbnail-exiting");

      await act(async () => {
        await vi.advanceTimersByTimeAsync(THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS + 16);
      });

      expect(cards[0]).not.toHaveClass("thumbnail-stack-shifting");
      expect(cards[0].style.getPropertyValue("--thumbnail-stack-shift")).toBe("");
      expect(cards[2]).toHaveClass("thumbnail-stack-shifting");
      expect(cards[2].style.getPropertyValue("--thumbnail-stack-shift")).toBe(
        `${-THUMBNAIL_CARD_SLOT_PX}px`,
      );
      expect(cards[2].style.translate).toBe(`0 ${-THUMBNAIL_CARD_SLOT_PX}px`);
    } finally {
      vi.useRealTimers();
    }
  });

  it("re-arms native preview hit testing as soon as deletion completes", async () => {
    let pointerPolls = 0;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: artifact.id };
      }
      if (command === "get_thumbnail_pointer_position") {
        pointerPolls += 1;
        return null;
      }
      return undefined;
    });

    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    await waitFor(() => expect(pointerPolls).toBeGreaterThan(0));
    vi.useFakeTimers();
    try {
      fireEvent.click(within(card).getByRole("button", { name: "Delete" }));
      expect(card).toHaveClass("thumbnail-exit-delete");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(3_201);
      });

      expect(vi.mocked(invoke)).toHaveBeenCalledWith("dismiss_artifact", {
        artifactId: artifact.id,
      });
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "refresh_thumbnail_interactivity",
      );
      const pollsAfterRemoval = pointerPolls;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1_000);
      });
      expect(pointerPolls).toBe(pollsAfterRemoval);
      const ignoreCalls = vi.mocked(invoke).mock.calls
        .filter(([command]) => command === "set_thumbnail_ignore_cursor_events");
      expect(ignoreCalls.at(-1)?.[1]).toEqual({ ignore: true });
    } finally {
      vi.useRealTimers();
    }
  });

  it("re-arms hit testing when a preview appears after the stack was empty", async () => {
    type CaptureCompletedHandler = (event: { payload: CaptureArtifact }) => void;
    type PointerSample = { x: number; y: number; inside: boolean };
    let onCaptureCompleted: CaptureCompletedHandler | null = null;
    const pointerResolver = {
      current: null as ((sample: PointerSample) => void) | null,
    };
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "capture-completed") {
        onCaptureCompleted = handler as CaptureCompletedHandler;
      }
      return () => undefined;
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: null };
      }
      if (command === "get_thumbnail_pointer_position") {
        return new Promise<PointerSample>((resolve) => {
          pointerResolver.current = resolve;
        });
      }
      return undefined;
    });
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => document.querySelector(".thumbnail-card")),
    });

    render(<Thumbnail />);
    await waitFor(() => expect(onCaptureCompleted).not.toBeNull());
    act(() => onCaptureCompleted?.({ payload: artifact }));
    await screen.findByRole("article");
    await waitFor(() => expect(pointerResolver.current).not.toBeNull());
    pointerResolver.current?.({ x: 40, y: 40, inside: true });

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "set_thumbnail_ignore_cursor_events",
        { ignore: false },
      );
    });
  });

  it("passes desktop clicks through as soon as the last preview starts exiting", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: artifact.id };
      }
      if (command === "get_thumbnail_pointer_position") return null;
      return undefined;
    });

    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    fireEvent.click(within(card).getByRole("button", { name: "Delete" }));
    expect(card).toHaveClass("thumbnail-exiting");
    expect(screen.getByRole("button", { name: "Minimize previews" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Minimize previews" }).closest(".thumbnail-stack-toolbar"))
      .toHaveClass("thumbnail-stack-toolbar-exiting");

    await waitFor(() => {
      const ignoreCalls = vi.mocked(invoke).mock.calls
        .filter(([command]) => command === "set_thumbnail_ignore_cursor_events");
      expect(ignoreCalls.at(-1)?.[1]).toEqual({ ignore: true });
    });
  });

  it("passes desktop clicks through as soon as the last saved preview is closed", async () => {
    const saved = {
      ...artifact,
      path: "/tmp/Captures_2026-07-19_18-00-00_000.png",
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [saved];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: saved.id };
      }
      if (command === "get_thumbnail_pointer_position") return null;
      return undefined;
    });

    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    fireEvent.click(within(card).getByRole("button", { name: "Close" }));
    expect(card).toHaveClass("thumbnail-exit-dismiss");
    expect(screen.getByRole("button", { name: "Minimize previews" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Minimize previews" }).closest(".thumbnail-stack-toolbar"))
      .toHaveClass("thumbnail-stack-toolbar-exiting");

    await waitFor(() => {
      const ignoreCalls = vi.mocked(invoke).mock.calls
        .filter(([command]) => command === "set_thumbnail_ignore_cursor_events");
      expect(ignoreCalls.at(-1)?.[1]).toEqual({ ignore: true });
    });
  });

  it("does not re-arm from null pointer polls while the last preview is exiting", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: artifact.id };
      }
      if (command === "get_thumbnail_pointer_position") return null;
      return undefined;
    });

    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    fireEvent.click(within(card).getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      const ignoreCalls = vi.mocked(invoke).mock.calls
        .filter(([command]) => command === "set_thumbnail_ignore_cursor_events");
      expect(ignoreCalls.at(-1)?.[1]).toEqual({ ignore: true });
    });

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 600));
    });

    const ignoreCalls = vi.mocked(invoke).mock.calls
      .filter(([command]) => command === "set_thumbnail_ignore_cursor_events");
    expect(ignoreCalls.at(-1)?.[1]).toEqual({ ignore: true });
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith("refresh_thumbnail_interactivity");
  });

  it("ignores an older pointer poll after deletion has re-armed the surviving previews", async () => {
    type PointerSample = { x: number; y: number; inside: boolean } | null;
    const pointerPolls: Array<(sample: PointerSample) => void> = [];
    let pointerTarget: Element | null = null;
    const secondArtifact = {
      ...artifact,
      id: "capture-2",
      preview_url: "captures-capture://artifact/capture-2",
      full_url: "captures-capture://artifact-full/capture-2",
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact, secondArtifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: secondArtifact.id };
      }
      if (command === "get_thumbnail_pointer_position") {
        return new Promise<PointerSample>((resolve) => pointerPolls.push(resolve));
      }
      return undefined;
    });
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => pointerTarget),
    });

    render(<Thumbnail />);
    const cards = await screen.findAllByRole("article");
    await waitFor(() => expect(pointerPolls).toHaveLength(1));

    fireEvent.click(within(cards[0]).getByRole("button", { name: "Delete" }));
    const deleteFinished = new Event("animationend", { bubbles: true });
    Object.defineProperty(deleteFinished, "animationName", {
      value: "thumbnail-delete",
    });
    fireEvent(cards[0], deleteFinished);
    await waitFor(() => expect(screen.getAllByRole("article")).toHaveLength(1));
    await waitFor(() => expect(pointerPolls.length).toBeGreaterThanOrEqual(2));

    // The completion poll sees the surviving card in its new position.
    pointerTarget = within(screen.getByRole("article")).getByRole("button", {
      name: "Delete",
    });
    await act(async () => {
      pointerPolls.at(-1)?.({ x: 40, y: 40, inside: true });
      await Promise.resolve();
    });

    // The poll that was already in flight before deletion now resolves over
    // the old, empty slot. It must not put the entire native window back into
    // click-through mode after the survivors were re-armed.
    pointerTarget = null;
    await act(async () => {
      pointerPolls[0]?.({ x: 40, y: 40, inside: true });
      await Promise.resolve();
    });

    const ignoreCalls = vi.mocked(invoke).mock.calls
      .filter(([command]) => command === "set_thumbnail_ignore_cursor_events");
    expect(ignoreCalls.at(-1)?.[1]).toEqual({ ignore: false });
  });

  it("serializes native hit-test updates so a delayed click-through cannot beat re-arming", async () => {
    const clickThroughGate = { release: null as (() => void) | null };
    let nativeClickThrough = false;
    let pointerPoll = 0;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_artifacts") return [artifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: artifact.id };
      }
      if (command === "get_thumbnail_pointer_position") {
        pointerPoll += 1;
        if (pointerPoll === 1) return { x: 40, y: 40, inside: true };
        return new Promise(() => undefined);
      }
      if (command === "set_thumbnail_ignore_cursor_events") {
        const ignore = Boolean((args as { ignore?: boolean } | undefined)?.ignore);
        if (ignore) {
          await new Promise<void>((resolve) => {
            clickThroughGate.release = () => {
              nativeClickThrough = true;
              resolve();
            };
          });
        } else {
          nativeClickThrough = false;
        }
      }
      return undefined;
    });
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => null),
    });

    render(<Thumbnail />);
    await screen.findByRole("article");
    await waitFor(() => expect(clickThroughGate.release).not.toBeNull());

    window.dispatchEvent(new Event("captures-thumbnail-hit-test-changed"));
    clickThroughGate.release?.();

    await waitFor(() => {
      const ignoreCalls = vi.mocked(invoke).mock.calls
        .filter(([command]) => command === "set_thumbnail_ignore_cursor_events");
      expect(ignoreCalls.at(-1)?.[1]).toEqual({ ignore: false });
    });
    expect(nativeClickThrough).toBe(false);
  });

  it("minimizes previews into a layered stack and expands them again", async () => {
    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const stack = card.closest(".thumbnail-stack")!;
    const minimize = screen.getByRole("button", { name: "Minimize previews" });
    expect(minimize).toHaveTextContent("Show less");
    expect(minimize).not.toHaveAttribute("data-tooltip");
    expect(screen.queryByRole("button", { name: "Clear previews" })).toBeNull();

    vi.useFakeTimers();
    fireEvent.click(minimize);
    expect(stack).toHaveClass("thumbnail-stack-minimizing");
    expect(stack).not.toHaveClass("thumbnail-stack-minimize-run");
    expect(stack).not.toHaveClass("thumbnail-stack-hover-ready");
    expect(minimize.closest(".thumbnail-stack-toolbar")).toHaveClass(
      "thumbnail-stack-toolbar-leaving",
    );
    expect(card).toHaveAttribute("aria-hidden", "true");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
    });
    expect(stack).toHaveClass("thumbnail-stack-minimize-run");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });

    expect(stack).toHaveClass("thumbnail-stack-minimized");
    expect(stack).not.toHaveClass("thumbnail-stack-minimizing");
    expect(card.querySelector("img")).toHaveAttribute("draggable", "false");
    expect(card.querySelector("img")).toHaveAttribute("hidden");
    expect(card.querySelector(".thumbnail-media")).toHaveStyle({
      backgroundImage: `url("${artifact.full_url}")`,
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
    });
    expect(stack).toHaveClass("thumbnail-stack-hover-ready");
    expect(screen.queryByRole("button", { name: "Minimize previews" })).toBeNull();
    const expand = screen.getByRole("button", { name: "Expand preview" });
    expect(expand).toHaveClass("thumbnail-collapsed-hit-target");
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "set_mini_previews_collapsed",
      { collapsed: true },
    );
    await act(async () => {
      fireEvent.click(expand);
      await Promise.resolve();
    });
    expect(stack).toHaveClass("thumbnail-stack-expanding");
    expect(screen.getByRole("button", { name: "Minimize previews" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Minimize previews" }).closest(".thumbnail-stack-toolbar"))
      .toHaveClass("thumbnail-stack-toolbar-entering");
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "set_mini_previews_collapsed",
      { collapsed: false },
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });

    expect(stack).not.toHaveClass("thumbnail-stack-compact");
    expect(card).not.toHaveAttribute("aria-hidden");
    expect(screen.getByRole("button", { name: "Minimize previews" })).toBeEnabled();
    expect(stack.contains(
      screen.getByRole("button", { name: "Minimize previews" }).closest(".thumbnail-stack-toolbar"),
    )).toBe(false);
  });

  it("reveals the newest capture and keeps Show less at the window after expanding", async () => {
    const stacked = Array.from({ length: 8 }, (_, index) => ({
      ...artifact,
      id: `capture-${index + 1}`,
    }));
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return stacked;
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: stacked.at(-1)?.id };
      }
      if (command === "get_thumbnail_pointer_position") {
        return new Promise(() => undefined);
      }
      return undefined;
    });

    render(<Thumbnail />);
    const cards = await screen.findAllByRole("article");
    expect(cards).toHaveLength(8);
    const stack = cards[0]!.closest(".thumbnail-stack")!;
    const viewportHeight = 400;
    Object.defineProperties(stack, {
      clientHeight: { configurable: true, value: viewportHeight },
    });
    const newestTop = thumbnailStackNewestScrollTop(8, viewportHeight);
    expect(newestTop).toBeGreaterThan(0);

    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });

    const collapsedCards = stack.querySelectorAll<HTMLElement>(":scope > .thumbnail-card");
    expect(collapsedCards).toHaveLength(8);
    collapsedCards.forEach((card, index) => {
      const depth = collapsedCards.length - index - 1;
      expect(card.style.getPropertyValue("--thumbnail-stack-base-depth")).toBe(
        String(depth),
      );
      expect(card.style.getPropertyValue("--thumbnail-stack-peek-jitter")).toBe(
        `${thumbnailStackPeekJitterPx(depth)}px`,
      );
      expect(card.style.getPropertyValue("--thumbnail-stack-hidden")).toBe("");
    });
    expect(collapsedCards[0].style.getPropertyValue("--thumbnail-stack-base-depth")).toBe("7");
    const restRots = [...collapsedCards].map((card) => (
      card.style.getPropertyValue("--thumbnail-stack-skew-rot")
    ));
    expect(restRots.at(-1)).toBe("0deg");
    expect(new Set(restRots.slice(0, -1)).size).toBe(7);

    const expand = screen.getByRole("button", { name: "Expand 8 previews" });
    expect(expand.style.getPropertyValue("--thumbnail-collapsed-skew-pad")).not.toBe("0px");
    expect(expand.style.getPropertyValue("--thumbnail-collapsed-skew-pad")).not.toBe("");
    stack.scrollTop = 0;
    await act(async () => {
      fireEvent.click(expand);
      await Promise.resolve();
    });
    expect(stack).toHaveClass("thumbnail-stack-expanding");
    expect(stack).toHaveClass("thumbnail-stack-scrollport");
    expect(screen.queryByRole("button", { name: "Show newer captures" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Show older captures" })).toBeNull();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });

    expect(stack.scrollTop).toBe(newestTop);
    expect(stack).toHaveClass("thumbnail-stack-scrollport");
    expect(screen.queryByRole("button", { name: "Show newer captures" })).toBeNull();
    expect(screen.getByRole("button", { name: "Show older captures" })).toBeInTheDocument();
    const restack = screen.getByRole("button", { name: "Minimize previews" })
      .closest(".thumbnail-stack-toolbar");
    expect(stack.contains(restack)).toBe(false);
  });

  it("fans collapsed previews with staggered tilt and settles that pose on expand", async () => {
    const secondArtifact = {
      ...artifact,
      id: "capture-2",
      preview_url: "captures-capture://artifact/capture-2",
      full_url: "captures-capture://artifact-full/capture-2",
    };
    const thirdArtifact = {
      ...artifact,
      id: "capture-3",
      preview_url: "captures-capture://artifact/capture-3",
      full_url: "captures-capture://artifact-full/capture-3",
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") {
        return [artifact, secondArtifact, thirdArtifact];
      }
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: thirdArtifact.id };
      }
      if (command === "get_thumbnail_pointer_position") {
        return new Promise(() => undefined);
      }
      return undefined;
    });

    render(<Thumbnail />);
    const cards = await screen.findAllByRole("article");
    const stack = cards[0].closest(".thumbnail-stack")!;
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
      await vi.advanceTimersByTimeAsync(32);
    });

    const oldest = thumbnailStackSkewCssVars("capture-1", 2);
    const middle = thumbnailStackSkewCssVars("capture-2", 1);
    const newest = thumbnailStackSkewCssVars("capture-3", 0);
    expect(cards[0].style.getPropertyValue("--thumbnail-stack-fan-tilt")).toBe(
      String(oldest["--thumbnail-stack-fan-tilt"]),
    );
    expect(cards[1].style.getPropertyValue("--thumbnail-stack-fan-tilt")).toBe(
      String(middle["--thumbnail-stack-fan-tilt"]),
    );
    expect(cards[2].style.getPropertyValue("--thumbnail-stack-fan-tilt")).toBe("0deg");
    expect(cards[0].style.getPropertyValue("--thumbnail-stack-skew-rot")).toBe(
      String(oldest["--thumbnail-stack-skew-rot"]),
    );
    expect(cards[1].style.getPropertyValue("--thumbnail-stack-skew-rot")).toBe(
      String(middle["--thumbnail-stack-skew-rot"]),
    );
    expect(cards[2].style.getPropertyValue("--thumbnail-stack-skew-rot")).toBe("0deg");
    expect(newest["--thumbnail-stack-skew-rot"]).toBe("0deg");
    expect(oldest["--thumbnail-stack-skew-rot"]).not.toBe(middle["--thumbnail-stack-skew-rot"]);

    const livePose = "matrix(0.97, 0.12, -0.12, 0.97, 10, -24)";
    const originalComputed = window.getComputedStyle.bind(window);
    const computedSpy = vi.spyOn(window, "getComputedStyle").mockImplementation((element) => {
      const style = originalComputed(element);
      if (element instanceof Element && element.classList.contains("thumbnail-card")) {
        return new Proxy(style, {
          get(target, prop, receiver) {
            if (prop === "transform") return livePose;
            return Reflect.get(target, prop, receiver);
          },
        });
      }
      return style;
    });

    const expand = screen.getByRole("button", { name: "Expand 3 previews" });
    expand.setAttribute("data-native-pointer-hover", "true");
    stack.classList.add("thumbnail-stack-hover-latched");
    await act(async () => {
      fireEvent.click(expand);
      await Promise.resolve();
    });
    expect(stack).toHaveClass("thumbnail-stack-expanding");
    expect(stack).not.toHaveClass("thumbnail-stack-expanding-from-hover");
    expect(cards[0].style.getPropertyValue("--thumbnail-stack-expand-from")).toBe(livePose);
    computedSpy.mockRestore();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });
    expect(cards[0].style.getPropertyValue("--thumbnail-stack-expand-from")).toBe("");
    expect(stack).not.toHaveClass("thumbnail-stack-compact");
  });

  it("drags the collapsed pile instead of expanding once the pointer moves", async () => {
    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const stack = card.closest(".thumbnail-stack")!;
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });
    vi.useRealTimers();

    const expand = screen.getByRole("button", { name: "Expand preview" });
    fireEvent.pointerDown(expand, {
      button: 0,
      pointerId: 1,
      screenX: 40,
      screenY: 80,
    });
    fireEvent.pointerMove(window, {
      pointerId: 1,
      screenX: 120,
      screenY: 40,
      bubbles: true,
    });
    await waitFor(() => {
      expect(stack).toHaveClass("thumbnail-stack-dragging");
    });
    expect(document.documentElement.style.getPropertyValue("--thumbnail-stack-drag-x")).toBe(
      "80px",
    );
    expect(document.documentElement.style.getPropertyValue("--thumbnail-stack-drag-y")).toBe(
      "-40px",
    );

    fireEvent.pointerUp(window, { pointerId: 1, bubbles: true });
    await waitFor(() => {
      expect(stack).not.toHaveClass("thumbnail-stack-dragging");
    });
    expect(stack).toHaveClass("thumbnail-stack-minimized");
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith(
      "set_mini_previews_collapsed",
      { collapsed: false },
    );
  });

  it("keeps Show less in the top gutter after the pile is dragged to the top", async () => {
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 800 });
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1200 });

    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const stack = card.closest(".thumbnail-stack")!;
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });
    vi.useRealTimers();

    const expand = screen.getByRole("button", { name: "Expand preview" });
    fireEvent.pointerDown(expand, {
      button: 0,
      pointerId: 1,
      screenX: 40,
      screenY: 700,
    });
    fireEvent.pointerMove(window, {
      pointerId: 1,
      screenX: 40,
      screenY: 40,
      bubbles: true,
    });
    await waitFor(() => {
      expect(stack).toHaveClass("thumbnail-stack-anchor-top");
    });
    fireEvent.pointerUp(window, { pointerId: 1, bubbles: true });
    await waitFor(() => {
      expect(stack).not.toHaveClass("thumbnail-stack-dragging");
    });

    fireEvent.click(expand);

    vi.useFakeTimers();
    await act(async () => {
      fireEvent.click(expand);
      await Promise.resolve();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });

    expect(stack).toHaveClass("thumbnail-stack-anchor-top");
    expect(stack).not.toHaveClass("thumbnail-stack-compact");
    const toolbar = screen.getByRole("button", { name: "Minimize previews" })
      .closest(".thumbnail-stack-toolbar");
    expect(toolbar).toHaveClass("thumbnail-stack-toolbar-anchor-top");
    expect(within(card).getByRole("button", { name: "Delete" })).toBeInTheDocument();
  });

  it("can drag the collapsed pile a second time without expanding first", async () => {
    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const stack = card.closest(".thumbnail-stack")!;
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });
    vi.useRealTimers();

    const expand = screen.getByRole("button", { name: "Expand preview" });
    expand.setPointerCapture = vi.fn();
    expand.releasePointerCapture = vi.fn();
    expand.hasPointerCapture = vi.fn(() => false);

    fireEvent.pointerDown(expand, {
      button: 0,
      pointerId: 1,
      screenX: 40,
      screenY: 80,
    });
    fireEvent.pointerMove(window, {
      pointerId: 1,
      screenX: 120,
      screenY: 40,
      bubbles: true,
    });
    await waitFor(() => {
      expect(document.documentElement.style.getPropertyValue("--thumbnail-stack-drag-x")).toBe(
        "80px",
      );
    });
    fireEvent.pointerUp(window, { pointerId: 1, bubbles: true });
    await waitFor(() => {
      expect(stack).not.toHaveClass("thumbnail-stack-dragging");
    });

    fireEvent.pointerDown(expand, {
      button: 0,
      pointerId: 1,
      screenX: 120,
      screenY: 40,
    });
    fireEvent.lostPointerCapture(expand, {
      pointerId: 1,
      buttons: 1,
      bubbles: true,
    });
    fireEvent.pointerMove(window, {
      pointerId: 1,
      screenX: 180,
      screenY: 10,
      bubbles: true,
    });
    await waitFor(() => {
      expect(stack).toHaveClass("thumbnail-stack-dragging");
    });
    expect(document.documentElement.style.getPropertyValue("--thumbnail-stack-drag-x")).toBe(
      "140px",
    );
    expect(document.documentElement.style.getPropertyValue("--thumbnail-stack-drag-y")).toBe(
      "-70px",
    );
    expect(expand.setPointerCapture).toHaveBeenCalled();
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith(
      "set_mini_previews_collapsed",
      { collapsed: false },
    );

    fireEvent.pointerUp(window, { pointerId: 1, bubbles: true });
    await waitFor(() => {
      expect(stack).not.toHaveClass("thumbnail-stack-dragging");
    });
    expect(stack).toHaveClass("thumbnail-stack-minimized");
  });

  it("opens downward after the collapsed pile is dragged to the top", async () => {
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 800 });
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1_280 });
    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const stack = card.closest(".thumbnail-stack")!;
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });
    vi.useRealTimers();

    const expand = screen.getByRole("button", { name: "Expand preview" });
    fireEvent.pointerDown(expand, {
      button: 0,
      pointerId: 7,
      screenX: 40,
      screenY: 80,
    });
    fireEvent.pointerMove(window, {
      pointerId: 7,
      screenX: 40,
      screenY: -400,
      bubbles: true,
    });
    await waitFor(() => {
      expect(stack).toHaveClass("thumbnail-stack-dragging");
    });
    fireEvent.pointerUp(window, { pointerId: 7, bubbles: true });
    await waitFor(() => {
      expect(stack).not.toHaveClass("thumbnail-stack-dragging");
    });
    expect(stack).toHaveClass("thumbnail-stack-anchor-top");

    // pointerDown always suppresses the following click so a drag does not
    // expand. Consume that skip, then click again to open the stack.
    fireEvent.click(expand);
    fireEvent.click(expand);
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Minimize previews" })
        .closest(".thumbnail-stack-toolbar")).toHaveClass("thumbnail-stack-toolbar-anchor-top");
    });
    expect(stack).toHaveClass("thumbnail-stack-anchor-top");
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 768 });
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1024 });
  });

  it("opens downward from a top-left preference", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: artifact.id };
      }
      if (command === "get_settings") {
        return { mini_preview_placement: "top_left" };
      }
      if (command === "get_thumbnail_pointer_position") {
        return new Promise(() => undefined);
      }
      return undefined;
    });
    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const stack = card.closest(".thumbnail-stack")!;
    await waitFor(() => {
      expect(stack).toHaveClass("thumbnail-stack-anchor-top");
    });
    expect(screen.getByRole("button", { name: "Minimize previews" })
      .closest(".thumbnail-stack-toolbar")).toHaveClass("thumbnail-stack-toolbar-anchor-top");
  });

  it("scrolls to the newest card when settings move an expanded pile between top and bottom", async () => {
    const stacked = Array.from({ length: 8 }, (_, index) => ({
      ...artifact,
      id: `capture-${index + 1}`,
    }));
    type SettingsHandler = (event: { payload: { mini_preview_placement: string } }) => void;
    let settingsChanged: SettingsHandler | undefined;
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "settings-changed") {
        settingsChanged = handler as SettingsHandler;
      }
      return () => undefined;
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return stacked;
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: stacked.at(-1)?.id };
      }
      if (command === "get_settings") {
        return { mini_preview_placement: "bottom_right" };
      }
      if (command === "get_thumbnail_pointer_position") {
        return new Promise(() => undefined);
      }
      return undefined;
    });

    render(<Thumbnail />);
    const cards = await screen.findAllByRole("article");
    expect(cards).toHaveLength(8);
    const stack = cards[0]!.closest(".thumbnail-stack")!;
    const viewportHeight = 400;
    Object.defineProperties(stack, {
      clientHeight: { configurable: true, value: viewportHeight },
    });
    const newestTop = thumbnailStackNewestScrollTop(8, viewportHeight);
    expect(newestTop).toBeGreaterThan(0);
    await waitFor(() => {
      expect(settingsChanged).toBeDefined();
    });

    stack.scrollTop = 40;
    await act(async () => {
      settingsChanged!({
        payload: { mini_preview_placement: "top_right" },
      });
    });

    await waitFor(() => {
      expect(stack).toHaveClass("thumbnail-stack-anchor-top");
    });
    expect(stack.scrollTop).toBe(0);
    expect(screen.getByRole("button", { name: "Minimize previews" })
      .closest(".thumbnail-stack-toolbar")).toHaveClass("thumbnail-stack-toolbar-anchor-top");
  });

  it("eases the hover fan closed before the dragged pile starts leaning", async () => {
    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const stack = card.closest(".thumbnail-stack")!;
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });

    const expand = screen.getByRole("button", { name: "Expand preview" });
    fireEvent.pointerDown(expand, {
      button: 0,
      pointerId: 3,
      screenX: 40,
      screenY: 80,
    });
    expect(stack).toHaveClass("thumbnail-stack-pressing");
    expect(stack).not.toHaveClass("thumbnail-stack-drag-sway");

    fireEvent.pointerMove(window, {
      pointerId: 3,
      screenX: 120,
      screenY: 40,
      bubbles: true,
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(stack).toHaveClass("thumbnail-stack-dragging");
    expect(stack).not.toHaveClass("thumbnail-stack-drag-sway");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(thumbnailStackFanCollapseMs(1) - 1);
    });
    expect(stack).not.toHaveClass("thumbnail-stack-drag-sway");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(stack).toHaveClass("thumbnail-stack-drag-sway");

    fireEvent.pointerUp(window, { pointerId: 3, bubbles: true });
    await act(async () => {
      await Promise.resolve();
    });
    expect(stack).not.toHaveClass("thumbnail-stack-dragging");
    expect(stack).not.toHaveClass("thumbnail-stack-drag-sway");
    expect(stack).not.toHaveClass("thumbnail-stack-pressing");
    vi.useRealTimers();
  });

  it("drops collapsed hover chrome after a drag ends away from the pile", async () => {
    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const stack = card.closest(".thumbnail-stack")!;
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });
    vi.useRealTimers();

    const expand = screen.getByRole("button", { name: "Expand preview" });
    expand.setPointerCapture = vi.fn();
    expand.releasePointerCapture = vi.fn();
    expand.hasPointerCapture = vi.fn(() => true);
    vi.spyOn(expand, "getBoundingClientRect").mockReturnValue({
      x: 20,
      y: 40,
      top: 40,
      left: 20,
      right: 220,
      bottom: 200,
      width: 200,
      height: 160,
      toJSON: () => ({}),
    });
    expand.setAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE, "true");

    fireEvent.pointerDown(expand, {
      button: 0,
      pointerId: 5,
      clientX: 80,
      clientY: 80,
      screenX: 40,
      screenY: 80,
    });
    fireEvent.pointerMove(window, {
      pointerId: 5,
      clientX: 280,
      clientY: 20,
      screenX: 240,
      screenY: 20,
      bubbles: true,
    });
    await waitFor(() => {
      expect(stack).toHaveClass("thumbnail-stack-dragging");
    });

    fireEvent.pointerUp(window, {
      pointerId: 5,
      clientX: 280,
      clientY: 20,
      bubbles: true,
    });
    await waitFor(() => {
      expect(stack).not.toHaveClass("thumbnail-stack-dragging");
    });

    expect(expand.releasePointerCapture).toHaveBeenCalledWith(5);
    expect(expand).not.toHaveAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE);
    expect(expand).toHaveAttribute(THUMBNAIL_HOVER_STALE_ATTRIBUTE, "true");
    expect(stack).toHaveClass("thumbnail-stack-minimized");

    fireEvent.pointerEnter(expand);
    expect(expand).not.toHaveAttribute(THUMBNAIL_HOVER_STALE_ATTRIBUTE);
  });

  it("cancels HTML5 dragstart on collapsed screenshots so the pile can move", async () => {
    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    vi.useFakeTimers();
    fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });
    vi.useRealTimers();

    const img = card.querySelector("img");
    expect(img).not.toBeNull();
    const event = new Event("dragstart", { bubbles: true, cancelable: true });
    img!.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });

  it("does not play card hover after expanding until the pointer moves", async () => {
    let pointer = { x: 40, y: 80, inside: true };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: artifact.id };
      }
      if (command === "get_thumbnail_pointer_position") return pointer;
      return undefined;
    });
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => document.querySelector(".thumbnail-card")),
    });

    render(<Thumbnail />);
    const card = await screen.findByRole("article");
    const stack = card.closest(".thumbnail-stack")!;
    vi.useFakeTimers();

    fireEvent.click(screen.getByRole("button", { name: "Minimize previews" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });
    expect(stack).toHaveClass("thumbnail-stack-minimized");

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Expand preview" }));
      await Promise.resolve();
    });
    expect(stack).toHaveClass("thumbnail-stack-expanding");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });

    expect(stack).not.toHaveClass("thumbnail-stack-compact");
    expect(stack).toHaveAttribute(THUMBNAIL_SUPPRESS_CARD_HOVER_ATTRIBUTE, "true");
    expect(card).not.toHaveAttribute("data-thumbnail-native-active");

    within(card).getByRole("button", { name: "Edit" }).focus();
    expect(stack).toHaveAttribute(THUMBNAIL_SUPPRESS_CARD_HOVER_ATTRIBUTE, "true");
    expect(card.matches(":focus-within")).toBe(true);

    pointer = { x: 88, y: 36, inside: true };
    await act(async () => {
      window.dispatchEvent(new PointerEvent("pointermove", {
        clientX: 88,
        clientY: 36,
      }));
      await vi.advanceTimersByTimeAsync(50);
    });

    expect(stack).not.toHaveAttribute(THUMBNAIL_SUPPRESS_CARD_HOVER_ATTRIBUTE);
    expect(card).toHaveAttribute("data-thumbnail-native-active", "true");
  });

  it("releases the overlay cursor over the hole left by a collapsed stack", async () => {
    let pointerReady = false;
    let pointerTarget: Element | null = null;
    const secondArtifact = {
      ...artifact,
      id: "capture-2",
      preview_url: "captures-capture://artifact/capture-2",
      full_url: "captures-capture://artifact-full/capture-2",
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return [artifact, secondArtifact];
      if (command === "get_clipboard_state") {
        return { revision: 0, artifact_id: secondArtifact.id };
      }
      if (command === "get_thumbnail_pointer_position") {
        return pointerReady ? { x: 40, y: 20, inside: true } : new Promise(() => undefined);
      }
      return undefined;
    });
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => pointerTarget),
    });

    render(<Thumbnail />);
    const cards = await screen.findAllByRole("article");
    const stack = cards[0].closest(".thumbnail-stack")!;
    const minimize = screen.getByRole("button", { name: "Minimize previews" });
    pointerTarget = minimize;
    pointerReady = true;
    window.dispatchEvent(new Event("captures-thumbnail-ready"));

    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "set_thumbnail_cursor",
        { kind: "pointer" },
      );
    });
    expect(document.documentElement).toHaveClass("thumbnail-native-tracking");

    vi.useFakeTimers();
    fireEvent.click(minimize);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(32);
      await vi.advanceTimersByTimeAsync(THUMBNAIL_STACK_EXPAND_COLLAPSE_MS);
    });
    vi.useRealTimers();

    expect(stack).toHaveClass("thumbnail-stack-minimized");
    pointerTarget = stack;
    window.dispatchEvent(new Event("captures-thumbnail-layout-changed"));

    await waitFor(() => {
      const ignoreCalls = vi.mocked(invoke).mock.calls
        .filter(([command]) => command === "set_thumbnail_ignore_cursor_events");
      expect(ignoreCalls.at(-1)?.[1]).toEqual({ ignore: true });
    });
    await waitFor(() => {
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "set_thumbnail_cursor",
        { kind: "default" },
      );
    });
    expect(document.documentElement).not.toHaveClass("thumbnail-native-tracking");
    expect(document.documentElement).not.toHaveAttribute("data-thumbnail-cursor");
    expect(document.documentElement.style.cursor).toBe("");
  });
});
