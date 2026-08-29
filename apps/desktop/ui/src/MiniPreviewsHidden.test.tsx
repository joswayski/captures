import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import {
  MiniPreviewsHiddenChip,
} from "./App";
import { miniPreviewsHiddenLabel } from "./lib/miniPreviewsHidden";
import type { CaptureArtifact } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}));

const artifacts: CaptureArtifact[] = [
  {
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
  },
  {
    id: "capture-2",
    path: null,
    preview_url: "captures-capture://artifact/capture-2",
    full_url: "captures-capture://artifact-full/capture-2",
    width: 800,
    height: 600,
    size_bytes: 80_000,
    created_at: "2026-07-19T18:01:00Z",
    mode: "region",
    history_saved: true,
    clipboard_copy_status: "skipped",
  },
];

describe("miniPreviewsHiddenLabel", () => {
  it("pluralizes parked preview counts", () => {
    expect(miniPreviewsHiddenLabel(0)).toBe("Previews");
    expect(miniPreviewsHiddenLabel(1)).toBe("1 preview");
    expect(miniPreviewsHiddenLabel(3)).toBe("3 previews");
  });
});

describe("MiniPreviewsHiddenChip", () => {
  beforeEach(() => {
    window.history.replaceState({}, "", "/?view=mini-previews-hidden&count=2");
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return artifacts;
      return undefined;
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("restores the stack from the parked count chip", async () => {
    render(<MiniPreviewsHiddenChip />);

    const chip = screen.getByRole("button", { name: "Show 2 previews" });
    expect(chip).toHaveTextContent("2 previews");
    fireEvent.click(chip);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("restore_mini_previews");
  });

  it("updates the count when the parked stack changes", async () => {
    type CountHandler = (event: { payload: number }) => void;
    const countHandler: { current: CountHandler | null } = { current: null };
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "mini-previews-hidden-count") {
        countHandler.current = handler as CountHandler;
      }
      return () => undefined;
    });

    render(<MiniPreviewsHiddenChip />);
    expect(screen.getByRole("button", { name: "Show 2 previews" })).toBeInTheDocument();

    await waitFor(() => expect(countHandler.current).not.toBeNull());
    countHandler.current?.({ payload: 1 });

    expect(await screen.findByRole("button", { name: "Show 1 preview" })).toBeInTheDocument();
  });
});
