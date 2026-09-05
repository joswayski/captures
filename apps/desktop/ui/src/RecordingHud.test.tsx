import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import {
  RecordingControlsHiddenNotice,
  RecordingHud,
  RecordingSavedNotice,
  StartupNotice,
} from "./App";
import type { RecordingSessionSnapshot } from "./types";

const { startDragging } = vi.hoisted(() => ({
  startDragging: vi.fn(async () => undefined),
}));
const { nativeMessage } = vi.hoisted(() => ({
  nativeMessage: vi.fn(async () => "Cancel"),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => true,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ startDragging }),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  message: nativeMessage,
}));

const baseSnapshot: RecordingSessionSnapshot = {
  id: "recording-1",
  state: "countdown",
  options: {
    kind: "video",
    target: {
      type: "region",
      display_id: "display-1",
      rect: { x: 10, y: 20, width: 800, height: 600 },
    },
    frames_per_second: 60,
    max_resolution: "original",
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
    gif: {
      max_width: 800,
      max_colors: 256,
      optimize: true,
    },
  },
  elapsed_ms: 0,
  countdown_remaining_seconds: 2,
  warning: null,
  error: null,
};

describe("RecordingHud", () => {
  let snapshot: RecordingSessionSnapshot;

  beforeEach(() => {
    snapshot = baseSnapshot;
    vi.mocked(listen).mockRejectedValue(new Error("event bridge unavailable"));
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_snapshot") return snapshot;
      if (command === "recording_controls_are_excluded") return true;
      if (command === "start_capture") return undefined;
      if (command === "hide_recording_hud") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("keeps status state current even when transient event listeners stall", async () => {
    vi.mocked(listen).mockImplementation(() => new Promise(() => undefined));
    render(<RecordingHud />);

    expect(await screen.findByText("0:00")).toBeInTheDocument();
    expect(screen.getByText("Starting…")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("get_recording_snapshot");
  });

  it("offers a screenshot action, compact tooltips, and background dragging", async () => {
    snapshot = {
      ...baseSnapshot,
      state: "recording",
      elapsed_ms: 4_000,
      countdown_remaining_seconds: null,
    };
    const { container } = render(<RecordingHud />);

    const screenshot = await screen.findByRole("button", { name: "Take a region screenshot" });
    fireEvent.click(screenshot);
    expect(container.querySelector(".recording-hud")).not.toHaveClass("recording-hud-capturing");
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("start_capture", { mode: "region" });
    });

    expect(screen.getByText("Recording")).toBeInTheDocument();
    expect(screen.queryByText("Not in recording")).not.toBeInTheDocument();
    const privacy = screen.getByText("These controls won’t show in recordings");
    expect(privacy).toBeInTheDocument();
    expect(container.querySelector(".recording-hud")?.firstElementChild).toBe(privacy);
    expect(container.querySelector(".recording-hud-main")).toContainElement(
      screen.getByRole("button", { name: "Hide recording controls" }),
    );
    expect(screen.getByRole("button", { name: "Hide recording controls" }))
      .not.toHaveAttribute("title");
    expect(screen.getAllByRole("tooltip").map((tooltip) => tooltip.textContent)).toEqual(expect.arrayContaining([
      "Stop and save",
      "Pause recording",
      "Restart recording",
      "Take a region screenshot",
      "Delete recording",
      "Hide controls",
    ]));
    expect(screen.queryByRole("button", { name: "Move recording controls" })).not.toBeInTheDocument();
    fireEvent.pointerDown(container.querySelector(".recording-hud")!, {
      button: 0,
    });
    expect(startDragging).toHaveBeenCalledOnce();
  });

  it("hides the controls without replacing them with a collapsed strip", async () => {
    snapshot = {
      ...baseSnapshot,
      state: "recording",
      elapsed_ms: 4_000,
      countdown_remaining_seconds: null,
    };
    render(<RecordingHud />);

    fireEvent.click(await screen.findByRole("button", { name: "Hide recording controls" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("hide_recording_hud", {
        sessionId: snapshot.id,
      });
    });
    expect(screen.getByRole("button", { name: "Stop recording" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Move recording controls" })).not.toBeInTheDocument();
  });

  it("explains how to restore controls after they are hidden", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_settings") {
        return {
          new_capture_shortcut: "Ctrl+Shift+Space",
        };
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<RecordingControlsHiddenNotice />);

    expect(screen.getByText("Recording controls hidden")).toBeInTheDocument();
    expect(screen.getByText(/Open Captures from the (menu bar|tray), or press/)).toBeInTheDocument();
    expect(await screen.findByText("Ctrl")).toBeInTheDocument();
    expect(screen.getByText("Shift")).toBeInTheDocument();
    expect(screen.getByText("Space")).toBeInTheDocument();
    expect(screen.getByText(/to bring them back/)).toBeInTheDocument();
    expect(screen.queryByText(/New Capture/)).not.toBeInTheDocument();
    expect(screen.queryByText(/your shortcut/)).not.toBeInTheDocument();
  });

  it("points first-run setup at the tray with a ready-to-use tooltip", () => {
    window.history.replaceState({}, "", "/?view=startup");

    render(<StartupNotice />);

    expect(screen.getByText("Captures is ready to use")).toBeInTheDocument();
    expect(screen.queryByText("Captures is here whenever you need it")).not.toBeInTheDocument();
    expect(screen.queryByText("Captures is running")).not.toBeInTheDocument();
    expect(screen.queryByText(/Use the (menu bar|tray) icon/)).not.toBeInTheDocument();
    expect(document.querySelector(".tray-notice-caret")).not.toBeInTheDocument();
  });

  it("renders a caret pointing at the tray when placement is provided", () => {
    window.history.replaceState({}, "", "/?view=startup&caret=top&caret_x=180");

    const { container } = render(<StartupNotice />);
    const notice = container.querySelector(".startup-notice");

    expect(screen.getByText("Captures is ready to use")).toBeInTheDocument();
    expect(notice).toHaveAttribute("data-caret", "top");
    expect((notice as HTMLElement | null)?.style.getPropertyValue("--tray-caret-x")).toBe(
      "180px",
    );
    expect(container.querySelector(".tray-notice-caret")).toBeInTheDocument();
  });

  it("offers history recovery wording until a recording is permanently saved", () => {
    render(<RecordingSavedNotice />);

    expect(screen.getByText("Recording ready")).toBeInTheDocument();
    expect(
      screen.getByText("Kept in Capture History for 30 days. Save a copy anytime."),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save file" })).toBeDisabled();
    expect(screen.queryByText(/Finder|Explorer/)).not.toBeInTheDocument();
  });

  it("updates the reusable saved notice for the latest recording", async () => {
    type SavedNoticeHandler = (event: {
      payload: { artifact_id: string; generation: number };
    }) => void;
    let savedNoticeHandler: SavedNoticeHandler | null = null;
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "recording-saved-artifact") {
        savedNoticeHandler = handler as SavedNoticeHandler;
      }
      return () => undefined;
    });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_recording_artifact") {
        const artifactId = (args as { artifactId?: string } | undefined)?.artifactId;
        if (artifactId === "recording-2") {
          return { id: artifactId, saved_path: "/Users/example/Captures/recording-2.mp4" };
        }
        return { id: artifactId, saved_path: null };
      }
      if (command === "reveal_recording_artifact") return undefined;
      if (command === "dismiss_recording_saved_notice") return undefined;
      return undefined;
    });
    render(<RecordingSavedNotice />);

    expect(savedNoticeHandler).not.toBeNull();
    act(() => {
      savedNoticeHandler!({
        payload: { artifact_id: "recording-2", generation: 2 },
      });
    });
    expect(await screen.findByRole("button", { name: "Show in Folder" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Show in Folder" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("reveal_recording_artifact", {
        artifactId: "recording-2",
      });
      expect(invoke).toHaveBeenCalledWith("dismiss_recording_saved_notice");
    });
  });

  it("warns when the platform cannot exclude recording controls", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_snapshot") return snapshot;
      if (command === "recording_controls_are_excluded") return false;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<RecordingHud />);

    expect(await screen.findByText(
      "These controls will show in recordings · Use Hide controls to keep them out",
    ))
      .toBeInTheDocument();
    expect(screen.queryByText("These controls won’t show in recordings"))
      .not.toBeInTheDocument();
  });

  it("updates the privacy menu text when the include preference changes", async () => {
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    let controlsExcluded = true;
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      handlers.set(event, handler as (event: { payload: unknown }) => void);
      return () => undefined;
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_snapshot") return snapshot;
      if (command === "recording_controls_are_excluded") return controlsExcluded;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<RecordingHud />);

    expect(await screen.findByText("These controls won’t show in recordings"))
      .toBeInTheDocument();

    controlsExcluded = false;
    await act(async () => {
      handlers.get("settings-changed")?.({
        payload: { include_recording_controls_in_captures: true },
      });
    });

    expect(await screen.findByText(
      "These controls will show in recordings · Use Hide controls to keep them out",
    ))
      .toBeInTheDocument();
  });

  it("uses a native Delete recording dialog before discarding", async () => {
    snapshot = {
      ...baseSnapshot,
      state: "recording",
      countdown_remaining_seconds: null,
    };
    nativeMessage.mockResolvedValueOnce("Delete");
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_snapshot") return snapshot;
      if (command === "recording_controls_are_excluded") return true;
      if (command === "discard_recording") return { ...snapshot, state: "discarded" };
      throw new Error(`unexpected command: ${command}`);
    });

    render(<RecordingHud />);
    fireEvent.click(await screen.findByRole("button", { name: "Delete recording" }));

    await waitFor(() => {
      expect(nativeMessage).toHaveBeenCalledWith(
        "This recording will be deleted permanently.",
        expect.objectContaining({
          title: "Delete recording?",
          buttons: { ok: "Delete", cancel: "Cancel" },
        }),
      );
      expect(invoke).toHaveBeenCalledWith("discard_recording", {
        sessionId: snapshot.id,
      });
    });
  });

  it("uses a native confirmation before restarting and deleting the current take", async () => {
    snapshot = {
      ...baseSnapshot,
      state: "recording",
      countdown_remaining_seconds: null,
    };
    nativeMessage.mockResolvedValueOnce("Restart");
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_snapshot") return snapshot;
      if (command === "recording_controls_are_excluded") return true;
      if (command === "restart_recording") return { ...snapshot, state: "countdown" };
      throw new Error(`unexpected command: ${command}`);
    });

    render(<RecordingHud />);
    fireEvent.click(await screen.findByRole("button", { name: "Restart recording" }));

    await waitFor(() => {
      expect(nativeMessage).toHaveBeenCalledWith(
        "The current recording will be deleted and a new countdown will begin.",
        expect.objectContaining({
          title: "Restart recording?",
          buttons: { ok: "Restart", cancel: "Cancel" },
        }),
      );
      expect(invoke).toHaveBeenCalledWith("restart_recording", {
        sessionId: snapshot.id,
      });
    });
  });

  it("does not present a failed recording as actively recording", async () => {
    snapshot = {
      ...baseSnapshot,
      state: "failed",
      countdown_remaining_seconds: null,
      error: "ScreenCaptureKit did not deliver a usable video frame",
    };
    const { container } = render(<RecordingHud />);

    expect(await screen.findByText("Failed")).toBeInTheDocument();
    expect(container.querySelector(".recording-hud-failed")).toBeInTheDocument();
  });
});
