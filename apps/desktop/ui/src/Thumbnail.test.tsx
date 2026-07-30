import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";

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
});
