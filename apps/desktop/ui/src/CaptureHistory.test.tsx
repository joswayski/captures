import { invoke } from "@tauri-apps/api/core";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { CaptureHistory, HistoryCard } from "./App";
import type { HistoryEntry, RecordingDraftManifest } from "./types";

vi.mock("@crabnebula/tauri-plugin-drag", () => ({ startDrag: vi.fn() }));

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
  saved_path: null,
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

const interruptedRecording: RecordingDraftManifest = {
  session_id: "62f30e4c-b28d-4029-aebb-592db83cbca9",
  created_at_ms: Date.parse("2026-07-19T20:00:00Z"),
  updated_at_ms: Date.parse("2026-07-19T20:00:05Z"),
  state: "failed",
  options: {
    kind: "video",
    target: { type: "display", display_id: "1" },
    frames_per_second: 30,
    max_resolution: "p1080",
    countdown_seconds: 3,
    show_cursor: true,
    highlight_clicks: false,
    show_keystrokes: false,
    audio: {
      capture_system_audio: false,
      microphone_device_id: null,
      mono_output: false,
      system_volume_percent: 100,
      microphone_volume_percent: 100,
      microphone_muted: false,
    },
    gif: { max_width: 800, max_colors: 256, optimize: true },
  },
  segments: [{
    index: 0,
    duration_ms: 4_200,
    size_bytes: 250_000,
    dropped_frames: 0,
    complete: true,
  }],
  final_path: null,
  last_error: "background task failed: recording failed: the recording did not contain a complete video frame",
};

describe("CaptureHistory", () => {
  beforeEach(() => {
    vi.mocked(startDrag).mockImplementation(async (_options, done) => {
      done?.({ result: "Dropped", cursorPos: { x: 40, y: 70 } });
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_capture_history") return [entry];
      if (command === "get_recording_drafts") return [];
      if (command === "restore_history_artifact") return undefined;
      if (command === "open_screenshot_editor") return undefined;
      if (command === "delete_history_artifact") return undefined;
      if (command === "clear_capture_history") return undefined;
      if (command === "open_recording_editor") return undefined;
      if (command === "reveal_recording_artifact") return undefined;
      if (command === "save_recording_artifact") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it.each([entry, recordingEntry, { ...recordingEntry, kind: "gif" as const }])(
    "drags a $kind file directly without restoring, opening, or deleting it",
    async (capture) => {
      const path = capture.kind === "screenshot" ? "/tmp/full-resolution.png" : `/tmp/media.${capture.kind === "gif" ? "gif" : "mp4"}`;
      vi.mocked(invoke).mockResolvedValue({ path, icon_path: "/tmp/drag-icon.png" });
      const deleted = vi.fn();
      render(<HistoryCard entry={capture} onDeleted={deleted} />);
      const preview = screen.getByRole("button", { name: /Open .* in editor/ });
      expect(preview).toHaveAttribute("draggable", "true");
      fireEvent.dragStart(preview);
      await waitFor(() => expect(startDrag).toHaveBeenCalledWith(
        { item: [path], icon: "/tmp/drag-icon.png", mode: "copy" }, expect.any(Function),
      ));
      await waitFor(() => expect(preview).toBeEnabled());
      expect(invoke).toHaveBeenCalledTimes(1);
      expect(invoke).toHaveBeenCalledWith("prepare_artifact_drag", { artifactId: capture.id });
      expect(deleted).not.toHaveBeenCalled();
      expect(preview).toBeInTheDocument();
    },
  );

  it("retains a cancelled drag and allows retry after preparation fails", async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error("file unavailable"))
      .mockResolvedValue({ path: "/tmp/original.png", icon_path: "/tmp/icon.png" });
    vi.mocked(startDrag).mockImplementation(async (_options, done) => {
      done?.({ result: "Cancelled", cursorPos: { x: 40, y: 70 } });
    });
    render(<HistoryCard entry={entry} onDeleted={vi.fn()} />);
    const preview = screen.getByRole("button", { name: "Open screenshot in editor" });
    fireEvent.dragStart(preview);
    expect(await screen.findByRole("alert")).toHaveTextContent("file unavailable");
    expect(startDrag).not.toHaveBeenCalled();
    fireEvent.dragStart(preview);
    await waitFor(() => expect(startDrag).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(preview).toBeEnabled());
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it("does not start a late drag after its History card is removed", async () => {
    let prepared!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementation(() => new Promise((resolve) => { prepared = resolve; }));
    const { unmount } = render(<HistoryCard entry={entry} onDeleted={vi.fn()} />);
    fireEvent.dragStart(screen.getByRole("button", { name: "Open screenshot in editor" }));
    unmount();
    await act(async () => prepared({ path: "/tmp/full.png", icon_path: "/tmp/icon.png" }));
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(startDrag).not.toHaveBeenCalled();
  });

  it.each([
    ["pointer release", () => fireEvent.pointerUp(window)],
    ["mouse release", () => fireEvent.mouseUp(window)],
    ["pointer cancellation", () => fireEvent.pointerCancel(window)],
    ["Escape", () => fireEvent.keyDown(window, { key: "Escape" })],
    ["lost window focus", () => fireEvent.blur(window)],
  ] as const)("ignores preparation completing after %s and preserves later clicks", async (_name, cancel) => {
    let prepared!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "prepare_artifact_drag") return new Promise((resolve) => { prepared = resolve; });
      if (command === "restore_history_artifact" || command === "open_screenshot_editor") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
    render(<HistoryCard entry={entry} onDeleted={vi.fn()} />);
    const preview = screen.getByRole("button", { name: "Open screenshot in editor" });
    fireEvent.pointerDown(preview, { button: 0, buttons: 1 });
    fireEvent.dragStart(preview);
    expect(preview).toBeDisabled();
    cancel(); // The disabled source need not receive this window-level event.
    await act(async () => prepared({ path: "/tmp/released.png", icon_path: "/tmp/icon.png" }));
    expect(startDrag).not.toHaveBeenCalled();
    expect(preview).toBeEnabled();
    fireEvent.click(preview, { detail: 1 }); // Release must not become an editor click.
    expect(invoke).toHaveBeenCalledTimes(1);
    fireEvent.pointerDown(preview);
    fireEvent.click(preview, { detail: 1 });
    await waitFor(() => expect(preview).toBeEnabled());
    fireEvent.click(preview, { detail: 0 }); // Keyboard activation has no pointer-down.
    await waitFor(() => expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "open_screenshot_editor")).toHaveLength(2));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("starts a delayed preparation only while its original gesture remains held", async () => {
    let prepared!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementation(() => new Promise((resolve) => { prepared = resolve; }));
    render(<HistoryCard entry={entry} onDeleted={vi.fn()} />);
    const preview = screen.getByRole("button", { name: "Open screenshot in editor" });
    fireEvent.pointerDown(preview, { button: 0, buttons: 1 });
    fireEvent.dragStart(preview);
    expect(startDrag).not.toHaveBeenCalled();
    await act(async () => prepared({ path: "/tmp/held.png", icon_path: "/tmp/icon.png" }));
    expect(startDrag).toHaveBeenCalledExactlyOnceWith(
      { item: ["/tmp/held.png"], icon: "/tmp/icon.png", mode: "copy" }, expect.any(Function),
    );
    expect(preview).toBeEnabled();
  });

  it.each(["resolve", "reject"] as const)("does not let an old preparation %s start or clear a newer held gesture", async (outcome) => {
    const preparations: { resolve: (value: unknown) => void; reject: (error: Error) => void }[] = [];
    vi.mocked(invoke).mockImplementation(() => new Promise((resolve, reject) => { preparations.push({ resolve, reject }); }));
    render(<HistoryCard entry={entry} onDeleted={vi.fn()} />);
    const preview = screen.getByRole("button", { name: "Open screenshot in editor" });
    fireEvent.pointerDown(preview, { button: 0, buttons: 1 });
    fireEvent.dragStart(preview);
    fireEvent.pointerUp(window);
    fireEvent.pointerDown(preview, { button: 0, buttons: 1 });
    fireEvent.dragStart(preview);
    expect(preparations).toHaveLength(2);
    await act(async () => {
      if (outcome === "resolve") preparations[0].resolve({ path: "/tmp/stale.png", icon_path: "/tmp/stale-icon.png" });
      else preparations[0].reject(new Error("stale failure"));
    });
    expect(startDrag).not.toHaveBeenCalled();
    expect(preview).toBeDisabled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    await act(async () => preparations[1].resolve({ path: "/tmp/current.png", icon_path: "/tmp/current-icon.png" }));
    expect(startDrag).toHaveBeenCalledExactlyOnceWith(
      { item: ["/tmp/current.png"], icon: "/tmp/current-icon.png", mode: "copy" }, expect.any(Function),
    );
    expect(preview).toBeEnabled();
  });

  it("hands release/cancel handling to the OS once the native drag has started", async () => {
    vi.mocked(invoke).mockResolvedValue({ path: "/tmp/full.png", icon_path: "/tmp/icon.png" });
    vi.mocked(startDrag).mockResolvedValue(undefined);
    render(<HistoryCard entry={entry} onDeleted={vi.fn()} />);
    const preview = screen.getByRole("button", { name: "Open screenshot in editor" });
    fireEvent.dragStart(preview);
    await waitFor(() => expect(startDrag).toHaveBeenCalledTimes(1));
    fireEvent.pointerUp(window);
    fireEvent.pointerCancel(window);
    fireEvent.blur(window);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(preview).toBeDisabled();
    act(() => vi.mocked(startDrag).mock.calls[0][1]?.({ result: "Cancelled", cursorPos: { x: 40, y: 70 } }));
    expect(preview).toBeEnabled();
  });

  it("suppresses the drag-release click but preserves the next click and keyboard activation", async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error("file unavailable"));
    render(<HistoryCard entry={entry} onDeleted={vi.fn()} />);
    const preview = screen.getByRole("button", { name: "Open screenshot in editor" });
    fireEvent.dragStart(preview);
    await screen.findByRole("alert");
    fireEvent.click(preview, { detail: 1 });
    expect(invoke).toHaveBeenCalledTimes(1);
    fireEvent.pointerDown(preview);
    fireEvent.click(preview, { detail: 1 });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("open_screenshot_editor", { artifactId: entry.id }));
    await waitFor(() => expect(preview).toBeEnabled());
    vi.mocked(invoke).mockRejectedValueOnce(new Error("file unavailable"));
    fireEvent.dragStart(preview);
    await screen.findByRole("alert");
    fireEvent.click(preview, { detail: 0 }); // Keyboard/assistive press has no pointer-down.
    await waitFor(() => expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "open_screenshot_editor")).toHaveLength(2));
  });

  it("does not drag missing media, action controls, or a second in-flight gesture", async () => {
    const { rerender } = render(<HistoryCard entry={{ ...recordingEntry, missing: true }} onDeleted={vi.fn()} />);
    const image = screen.getByRole("img");
    expect(image.closest("button")).toHaveAttribute("draggable", "false");
    fireEvent.dragStart(image.closest("button")!);
    expect(invoke).not.toHaveBeenCalled();
    rerender(<HistoryCard entry={entry} onDeleted={vi.fn()} />);
    for (const name of ["Edit", "Restore", "Delete from History"]) {
      fireEvent.dragStart(screen.getByRole("button", { name }));
    }
    expect(invoke).not.toHaveBeenCalled();
    vi.mocked(invoke).mockImplementation(() => new Promise(() => {}));
    const preview = screen.getByRole("button", { name: "Open screenshot in editor" });
    fireEvent.dragStart(preview);
    fireEvent.dragStart(preview);
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("restores a capture and requires confirmation before permanent deletion", async () => {
    render(<CaptureHistory />);

    expect(await screen.findByRole("heading", { name: "Capture History" })).toBeInTheDocument();
    expect(screen.getByText("On this device")).toBeInTheDocument();
    expect(screen.queryByText(/on this mac/i)).not.toBeInTheDocument();
    expect(screen.getByText("1440 × 900 · 250 KB")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Restore" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("restore_history_artifact", { artifactId: entry.id });
    });
    expect(await screen.findByRole("button", { name: "Restored" })).toBeInTheDocument();

    const deleteButton = screen.getByRole("button", { name: "Delete from History" });
    expect(deleteButton).toHaveClass("history-delete");
    expect(deleteButton).not.toHaveClass("history-delete-confirm");
    expect(deleteButton.parentElement).toHaveClass("history-image-wrap");
    expect(document.querySelector(".history-actions .history-delete")).toBeNull();

    fireEvent.click(deleteButton);
    expect(screen.getByRole("button", { name: "Confirm permanent deletion" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Confirm permanent deletion" }))
      .toHaveClass("history-delete-confirm");
    expect(invoke).not.toHaveBeenCalledWith("delete_history_artifact", expect.anything());

    fireEvent.click(screen.getByRole("button", { name: "Confirm permanent deletion" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("delete_history_artifact", { artifactId: entry.id });
    });
  });

  it("requires confirmation before deleting every capture at once", async () => {
    render(<CaptureHistory />);

    expect(await screen.findByRole("button", { name: "Delete all captures" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Delete all captures" }));
    expect(screen.getByRole("button", { name: "Confirm delete all captures" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel delete all captures" })).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("clear_capture_history");

    fireEvent.click(screen.getByRole("button", { name: "Cancel delete all captures" }));
    expect(screen.getByRole("button", { name: "Delete all captures" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel delete all captures" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Delete all captures" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm delete all captures" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("clear_capture_history");
    });
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Delete all captures" })).not.toBeInTheDocument();
      expect(screen.queryByText("1440 × 900 · 250 KB")).not.toBeInTheDocument();
    });
  });

  it("opens a screenshot from its preview before opening the editor", async () => {
    render(<CaptureHistory />);

    fireEvent.click(await screen.findByRole("button", { name: "Open screenshot in editor" }));

    await waitFor(() => {
      const restoreCall = vi.mocked(invoke).mock.calls.findIndex(
        ([command]) => command === "restore_history_artifact",
      );
      const editorCall = vi.mocked(invoke).mock.calls.findIndex(
        ([command]) => command === "open_screenshot_editor",
      );
      expect(restoreCall).toBeGreaterThanOrEqual(0);
      expect(editorCall).toBeGreaterThan(restoreCall);
    });
  });

  it("restores a historical screenshot before opening it in the editor", async () => {
    render(<CaptureHistory />);

    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));

    await waitFor(() => {
      const restoreCall = vi.mocked(invoke).mock.calls.findIndex(
        ([command]) => command === "restore_history_artifact",
      );
      const editorCall = vi.mocked(invoke).mock.calls.findIndex(
        ([command]) => command === "open_screenshot_editor",
      );
      expect(restoreCall).toBeGreaterThanOrEqual(0);
      expect(editorCall).toBeGreaterThan(restoreCall);
      expect(invoke).toHaveBeenCalledWith("open_screenshot_editor", {
        artifactId: entry.id,
      });
    });
  });

  it("lets history-only recordings save permanently and delete from history", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_capture_history") return [recordingEntry];
      if (command === "get_recording_drafts") return [];
      if (command === "open_recording_editor") return undefined;
      if (command === "save_recording_artifact") return undefined;
      if (command === "delete_history_artifact") return undefined;
      if (command === "clear_capture_history") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
    render(<CaptureHistory />);

    expect(await screen.findByText("1920 × 1080 · 5.0 MB · 1:02")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("open_recording_editor", { artifactId: recordingEntry.id });
    });
    fireEvent.click(screen.getByRole("button", { name: "Save file" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("save_recording_artifact", { artifactId: recordingEntry.id });
    });

    fireEvent.click(screen.getByRole("button", { name: "Delete from History" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm permanent deletion" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("delete_history_artifact", { artifactId: recordingEntry.id });
    });
  });

  it("shows permanently saved recordings in their folder", async () => {
    const savedRecording = {
      ...recordingEntry,
      saved_path: "/Users/example/Captures/recording.mp4",
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_capture_history") return [savedRecording];
      if (command === "get_recording_drafts") return [];
      if (command === "reveal_recording_artifact") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
    render(<CaptureHistory />);

    fireEvent.click(await screen.findByRole("button", { name: "Show in Folder" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_recording_artifact", {
        artifactId: savedRecording.id,
      });
    });
  });

  it("offers one direct removal action when a recording file is already missing", async () => {
    const missingRecording = { ...recordingEntry, missing: true };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_capture_history") return [missingRecording];
      if (command === "get_recording_drafts") return [];
      if (command === "delete_history_artifact") return undefined;
      if (command === "clear_capture_history") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
    const { container } = render(<CaptureHistory />);

    expect(await screen.findByText("File missing")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Show in Folder" })).not.toBeInTheDocument();
    expect(container.querySelector(".history-actions .history-delete")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Remove missing entry" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("delete_history_artifact", {
        artifactId: missingRecording.id,
      });
    });
    expect(screen.queryByRole("button", { name: "Confirm removal from History" }))
      .not.toBeInTheDocument();
  });

  it("shows interrupted recordings inside Capture History", async () => {
    let drafts = [interruptedRecording];
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_capture_history") return [];
      if (command === "get_recording_drafts") return drafts;
      if (command === "recover_recording_draft") {
        drafts = [];
        return undefined;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<CaptureHistory />);

    expect(await screen.findByRole("heading", { name: "Interrupted recordings" })).toBeInTheDocument();
    expect(screen.getByText(/0:04 recovered so far/)).toBeInTheDocument();
    expect(screen.getByText("The recording did not contain a complete video frame")).toBeInTheDocument();
    expect(screen.queryByText(/background task failed/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete all captures" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Recover" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("recover_recording_draft", {
        sessionId: interruptedRecording.session_id,
      });
    });
    await waitFor(() => {
      expect(screen.queryByRole("heading", { name: "Interrupted recordings" })).not.toBeInTheDocument();
    });
  });
});
