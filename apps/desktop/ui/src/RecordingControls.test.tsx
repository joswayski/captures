import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useState } from "react";

import { RecordingCountdown, RecordingRegionIndicator, ScreenshotCountdown } from "./App";
import { CustomSelect, placeCustomSelectMenu } from "./CustomSelect";
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

function DropdownHarness({
  className,
  triggerLabel,
}: {
  className?: string;
  triggerLabel?: string;
} = {}) {
  const [value, setValue] = useState("one");
  return (
    <CustomSelect
      value={value}
      ariaLabel="Quality"
      className={className}
      triggerLabel={triggerLabel}
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

  it("can show a compact trigger label without changing option names", () => {
    render(<DropdownHarness className="filename-format-select" triggerLabel=".png" />);
    const trigger = screen.getByRole("combobox", { name: "Quality" });
    expect(trigger).toHaveTextContent(".png");
    expect(trigger.closest(".custom-select")).toHaveClass("filename-format-select");

    fireEvent.click(trigger);
    const listbox = screen.getByRole("listbox", { name: "Quality" });
    expect(listbox).toHaveClass("filename-format-select-listbox");
    fireEvent.click(screen.getByRole("option", { name: "Two" }));
    expect(trigger).toHaveTextContent(".png");
  });

  it("portals the listbox so overflow-hidden ancestors cannot clip it", () => {
    render(
      <div style={{ overflow: "hidden", width: 80 }}>
        <DropdownHarness />
      </div>,
    );
    fireEvent.click(screen.getByRole("combobox", { name: "Quality" }));
    const listbox = screen.getByRole("listbox", { name: "Quality" });
    expect(listbox.parentElement).toBe(document.body);
    expect(listbox).toHaveStyle({ position: "fixed" });
  });
});

describe("placeCustomSelectMenu", () => {
  it("opens above when the menu would cross the bottom of the display", () => {
    const layout = placeCustomSelectMenu(
      { top: 720, left: 200, right: 420, bottom: 754, width: 220, height: 34 },
      { width: 280, height: 160 },
      { width: 1280, height: 800 },
      4,
    );
    expect(layout.placement).toBe("above");
    expect(layout.top).toBe(720 - 6 - 160);
    expect(layout.left).toBe(140);
    expect(layout.minWidth).toBe(220);
  });

  it("shifts right when a right-aligned menu would clip the left edge", () => {
    const layout = placeCustomSelectMenu(
      { top: 100, left: 12, right: 120, bottom: 134, width: 108, height: 34 },
      { width: 320, height: 180 },
      { width: 800, height: 600 },
      4,
    );
    expect(layout.left).toBe(8);
    expect(layout.placement).toBe("below");
  });

  it("shifts left when the menu would clip the right edge", () => {
    const layout = placeCustomSelectMenu(
      { top: 200, left: 700, right: 792, bottom: 234, width: 92, height: 34 },
      { width: 320, height: 120 },
      { width: 800, height: 600 },
      3,
    );
    expect(layout.left).toBe(472);
  });

  it("keeps a bottom-of-window menu inside the viewport", () => {
    const layout = placeCustomSelectMenu(
      { top: 540, left: 80, right: 280, bottom: 574, width: 200, height: 34 },
      { width: 280, height: 180 },
      { width: 800, height: 600 },
      3,
    );
    expect(layout.placement).toBe("above");
    expect(layout.top).toBeGreaterThanOrEqual(8);
    expect(layout.top + Math.min(layout.maxHeight, 180)).toBeLessThanOrEqual(592);
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
    const { container } = render(<RecordingCountdown />);

    expect(await screen.findByText("3", { selector: "strong" })).toBeInTheDocument();
    expect(container.querySelector(".recording-countdown-steps")).not.toBeInTheDocument();
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

  it("fades the full-display countdown when recording starts", async () => {
    const { container } = render(<RecordingCountdown />);
    await screen.findByText("3", { selector: "strong" });

    await act(async () => {
      handlers.get("recording-countdown")?.({
        payload: { session_id: countdownSnapshot.id, remaining_seconds: 1 },
      });
      handlers.get("recording-state-changed")?.({
        payload: {
          ...countdownSnapshot,
          state: "recording",
          countdown_remaining_seconds: null,
        },
      });
    });

    expect(container.querySelector(".recording-countdown")).toHaveClass("exiting");
    expect(screen.getByText("1", { selector: "strong" })).toBeInTheDocument();
    expect(screen.queryByText("3", { selector: "strong" })).not.toBeInTheDocument();
  });
});

describe("RecordingRegionIndicator", () => {
  afterEach(() => {
    window.history.replaceState({}, "", "/");
  });

  it("keeps the selected region clear while shading and framing its exterior", () => {
    window.history.replaceState(
      {},
      "",
      "/?view=recording-region-indicator&x=120&y=80&width=640&height=360",
    );

    const { container } = render(<RecordingRegionIndicator />);

    expect(container.querySelector(".capture-shade-full")).toHaveStyle({
      clipPath: expect.stringContaining("120px 80px"),
    });
    expect(container.querySelector(".recording-region-indicator-frame")).toHaveStyle({
      left: "120px",
      top: "80px",
      width: "640px",
      height: "360px",
    });
  });

  it("stays transparent when its native window has no valid region", () => {
    window.history.replaceState({}, "", "/?view=recording-region-indicator");

    const { container } = render(<RecordingRegionIndicator />);

    expect(container.querySelector(".capture-shade-full")).not.toBeInTheDocument();
    expect(container.querySelector(".recording-region-indicator-frame")).not.toBeInTheDocument();
  });
});

describe("ScreenshotCountdown", () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();

  beforeEach(() => {
    handlers.clear();
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      handlers.set(event, handler as (event: { payload: unknown }) => void);
      return () => undefined;
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_screenshot_countdown") return { remaining_seconds: 3 };
      if (command === "cancel_screenshot_countdown") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("updates the full-screen countdown and lets Escape cancel", async () => {
    render(<ScreenshotCountdown />);

    expect(await screen.findByText("Screenshot in")).toBeInTheDocument();
    expect(await screen.findByText("3", { selector: "strong" })).toBeInTheDocument();
    await act(async () => {
      handlers.get("screenshot-countdown")?.({
        payload: { remaining_seconds: 3 },
      });
    });
    expect(screen.getByText("3", { selector: "strong" })).toBeInTheDocument();

    await act(async () => {
      handlers.get("screenshot-countdown")?.({
        payload: { remaining_seconds: 2 },
      });
    });
    expect(screen.getByText("2", { selector: "strong" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("cancel_screenshot_countdown");
    });
  });
});
