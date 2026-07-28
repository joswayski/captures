import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useState } from "react";

import { CustomSelect, RecordingCountdown } from "./App";
import type { RecordingSessionSnapshot } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  message: vi.fn(),
}));

function DropdownHarness() {
  const [value, setValue] = useState("one");
  return (
    <CustomSelect
      value={value}
      ariaLabel="Quality"
      options={[
        { value: "one", label: "One" },
        { value: "two", label: "Two" },
        { value: "three", label: "Three" },
      ]}
      onChange={setValue}
    />
  );
}

const countdownSnapshot: RecordingSessionSnapshot = {
  id: "recording-1",
  state: "countdown",
  options: {
    kind: "video",
    target: { type: "display", display_id: "display-2" },
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
    gif: { max_width: 800, max_colors: 256, optimize: true },
  },
  elapsed_ms: 0,
  countdown_remaining_seconds: 3,
  warning: null,
  error: null,
};

describe("CustomSelect", () => {
  it("supports arrow navigation, selection, Escape, and outside-click dismissal", () => {
    render(
      <div onPointerDown={(event) => event.stopPropagation()}>
        <DropdownHarness />
        <button type="button">Outside</button>
      </div>,
    );
    const trigger = screen.getByRole("combobox", { name: "Quality" });

    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    fireEvent.keyDown(trigger, { key: "Enter" });
    expect(trigger).toHaveTextContent("Two");
    expect(trigger).toHaveAttribute("aria-expanded", "false");

    fireEvent.click(trigger);
    fireEvent.keyDown(trigger, { key: "Escape" });
    expect(trigger).toHaveAttribute("aria-expanded", "false");

    fireEvent.click(trigger);
    fireEvent.pointerDown(screen.getByRole("button", { name: "Outside" }));
    expect(trigger).toHaveAttribute("aria-expanded", "false");

    fireEvent.keyDown(trigger, { key: " " });
    expect(trigger).toHaveAttribute("aria-expanded", "true");
  });

  it("opens above the trigger when the menu would cross the display edge", () => {
    render(<DropdownHarness />);
    const trigger = screen.getByRole("combobox", { name: "Quality" });
    vi.spyOn(trigger, "getBoundingClientRect").mockReturnValue({
      x: 200,
      y: 720,
      top: 720,
      left: 200,
      right: 420,
      bottom: 754,
      width: 220,
      height: 34,
      toJSON: () => undefined,
    });

    fireEvent.click(trigger);

    expect(trigger.closest(".custom-select")).toHaveClass("open-above");
    expect(screen.getByRole("listbox", { name: "Quality" })).toHaveStyle({
      maxHeight: "240px",
    });
  });
});

describe("RecordingCountdown", () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();

  beforeEach(() => {
    handlers.clear();
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      handlers.set(event, handler as (event: { payload: unknown }) => void);
      return () => undefined;
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_recording_snapshot") return countdownSnapshot;
      if (command === "discard_recording") {
        return { ...countdownSnapshot, state: "discarded" };
      }
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("updates the full-screen countdown and lets Escape cancel the session", async () => {
    render(<RecordingCountdown />);

    expect(await screen.findByText("3", { selector: "strong" })).toBeInTheDocument();
    await act(async () => {
      handlers.get("recording-countdown")?.({
        payload: { session_id: countdownSnapshot.id, remaining_seconds: 2 },
      });
    });
    expect(screen.getByText("2", { selector: "strong" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("discard_recording", {
        sessionId: countdownSnapshot.id,
      });
      expect(vi.mocked(invoke).mock.calls.filter(([command]) => (
        command === "discard_recording"
      ))).toHaveLength(1);
    });
  });
});
