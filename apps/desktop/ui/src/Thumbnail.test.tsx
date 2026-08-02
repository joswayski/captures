import { invoke } from "@tauri-apps/api/core";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

import { Thumbnail } from "./App";
import type { CaptureArtifact } from "./types";

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
    document.documentElement.classList.remove("thumbnail-native-tracking");
    Reflect.deleteProperty(document, "elementFromPoint");
    vi.clearAllMocks();
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
    Object.defineProperties(stack, {
      clientHeight: { configurable: true, value: 400 },
      scrollHeight: { configurable: true, value: 1_000 },
    });
    stack.scrollTop = 600;

    fireEvent.scroll(stack);

    const older = await screen.findByRole("button", { name: "Show older captures" });
    expect(screen.queryByRole("button", { name: "Show newer captures" })).toBeNull();
    const scrollTo = vi.fn();
    Object.defineProperty(stack, "scrollTo", {
      configurable: true,
      value: scrollTo,
    });

    fireEvent.click(older);

    expect(scrollTo).toHaveBeenCalledWith({ top: 416, behavior: "smooth" });

    stack.scrollTop = 300;
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
    const firstDelete = within(cards[0]).getByRole("button", { name: "Delete" });
    const secondDelete = within(cards[1]).getByRole("button", { name: "Delete" });
    pointerReady = true;
    pointerTarget = firstDelete;

    fireEvent.click(firstDelete);

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
  });

  it("re-arms native preview hit testing as soon as deletion completes", async () => {
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
    } finally {
      vi.useRealTimers();
    }
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
});
