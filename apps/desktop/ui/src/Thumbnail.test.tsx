import { invoke } from "@tauri-apps/api/core";
import { render, screen } from "@testing-library/react";

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
    vi.clearAllMocks();
  });

  it("preserves the native hover presentation while focus moves to a viewer", async () => {
    render(<Thumbnail />);

    const card = await screen.findByRole("article");
    document.documentElement.classList.add("thumbnail-native-tracking");
    card.classList.add("thumbnail-card-native-active");

    window.dispatchEvent(new Event("focus"));

    expect(document.documentElement).toHaveClass("thumbnail-native-tracking");
    expect(card).toHaveClass("thumbnail-card-native-active");
  });
});
