import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { CaptureHistory } from "./App";
import type { HistoryEntry } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}));

const entry: HistoryEntry = {
  id: "7e3191ca-8596-4d22-a6e1-4b57a64f00cb",
  kind: "screenshot",
  preview_url: "captures-capture://history-preview/capture-1",
  full_url: "captures-capture://history-full/capture-1",
  width: 1_440,
  height: 900,
  size_bytes: 250_000,
  created_at: "2026-07-19T18:00:00Z",
  mode: "region",
};

const recordingEntry: HistoryEntry = {
  id: "8d11b283-3ac8-4510-8780-4910a7ed4305",
  kind: "video",
  poster_url: "captures-capture://localhost/poster/recording-1",
  media_url: "captures-capture://localhost/media/recording-1",
  saved_path: "/Users/example/Captures/recording.mp4",
  mime_type: "video/mp4",
  duration_ms: 62_500,
  width: 1_920,
  height: 1_080,
      size_bytes: 5_000_000,
      dropped_frames: 0,
  has_system_audio: true,
  has_microphone_audio: true,
  created_at: "2026-07-19T19:00:00Z",
  target: { type: "display", display_id: "1" },
  missing: false,
};

describe("CaptureHistory", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_capture_history") return [entry];
      if (command === "restore_history_artifact") return undefined;
      if (command === "delete_history_artifact") return undefined;
      if (command === "open_recording_editor") return undefined;
      if (command === "reveal_recording_artifact") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("restores a capture and requires confirmation before permanent deletion", async () => {
    render(<CaptureHistory />);

    expect(await screen.findByRole("heading", { name: "Capture History" })).toBeInTheDocument();
    expect(screen.getByText("1440 × 900 · 250 KB")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Restore" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("restore_history_artifact", { artifactId: entry.id });
    });
    expect(await screen.findByRole("button", { name: "Restored" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Delete from History" }));
    expect(screen.getByRole("button", { name: "Confirm permanent deletion" })).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("delete_history_artifact", expect.anything());

    fireEvent.click(screen.getByRole("button", { name: "Confirm permanent deletion" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("delete_history_artifact", { artifactId: entry.id });
    });
  });

  it("opens recording entries without duplicating or deleting their media", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_capture_history") return [recordingEntry];
      if (command === "open_recording_editor") return undefined;
      if (command === "reveal_recording_artifact") return undefined;
      if (command === "delete_history_artifact") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
    render(<CaptureHistory />);

    expect(await screen.findByText("1920 × 1080 · 5.0 MB · 1:02")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("open_recording_editor", { artifactId: recordingEntry.id });
    });
    fireEvent.click(screen.getByRole("button", { name: "Reveal" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_recording_artifact", { artifactId: recordingEntry.id });
    });

    fireEvent.click(screen.getByRole("button", { name: "Remove from History" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm removal from History" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("delete_history_artifact", { artifactId: recordingEntry.id });
    });
  });
});
