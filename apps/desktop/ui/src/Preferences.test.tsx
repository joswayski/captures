import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";

import { Preferences } from "./App";
import { detectShortcutPlatform, platformShortcutHelp } from "./lib/shortcut";
import type { AppSettings, UpdateStatus } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(async () => () => undefined),
}));

const settings: AppSettings = {
  appearance: "system",
  theme: "mustard",
  custom_theme: {
    accent: "#ffca28",
    signal: "#ef4650",
  },
  output_directory: "/Users/josevalerio/Captures",
  new_capture_shortcut: "Ctrl+Shift+Space",
  region_shortcut: "Ctrl+Shift+4",
  window_shortcut: "Ctrl+Shift+W",
  display_shortcut: "Ctrl+Shift+3",
  auto_copy_to_clipboard: true,
  auto_start_on_selection: false,
  show_mini_previews: true,
  mini_preview_placement: "bottom_left",
  include_mini_previews_in_captures: false,
  include_recording_controls_in_captures: false,
  launch_at_login: false,
  last_screen_permission_request_id: null,
  pending_capture_after_restart: null,
  onboarding_completed: true,
  screenshot_countdown_seconds: 0,
  freeze_screen: true,
  show_cursor_in_screenshots: true,
  screenshot_format: "png",
  show_update_changelog: true,
  recording: {
    video_shortcut: "Ctrl+Shift+5",
    gif_shortcut: "Ctrl+Shift+6",
    video_format: "mp4",
    video_fps: 30,
    video_max_resolution: "p1080",
    gif_fps: 15,
    gif_max_width: 800,
    gif_max_colors: 256,
    countdown_seconds: 3,
    show_cursor: true,
    capture_system_audio: false,
    microphone_device_id: null,
    mono_audio: false,
    highlight_clicks: false,
    show_keystrokes: false,
    open_editor_after_recording: true,
  },
};

const originalScrollIntoView = Object.getOwnPropertyDescriptor(
  HTMLElement.prototype,
  "scrollIntoView",
);

describe("Preferences", () => {
  beforeEach(() => {
    vi.mocked(listen).mockImplementation(async () => () => undefined);
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_settings") return settings;
      if (command === "get_update_status") {
        return { state: "idle", current_version: "0.1.0", current_display_version: "0.1.0" };
      }
      if (command === "set_shortcut_capture_suppressed") return undefined;
      if (command === "update_settings") return (args as { settings: AppSettings }).settings;
      if (command === "open_capture_history") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
    document.documentElement.removeAttribute("data-capture-theme");
    document.documentElement.removeAttribute("style");
    window.localStorage.clear();
    window.history.replaceState({}, "", "/");
    if (originalScrollIntoView) {
      Object.defineProperty(HTMLElement.prototype, "scrollIntoView", originalScrollIntoView);
    } else {
      delete (HTMLElement.prototype as Partial<HTMLElement>).scrollIntoView;
    }
  });

  it("opens Capture History from Preferences", async () => {
    render(<Preferences />);

    fireEvent.click(await screen.findByRole("button", { name: "Capture History…" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("open_capture_history");
    });
  });

  it("does not offer a global feedback shortcut", async () => {
    render(<Preferences />);

    fireEvent.click(await screen.findByRole("button", { name: "Shortcuts" }));
    await screen.findByRole("heading", { name: "Shortcuts" });
    expect(screen.queryByText("Send Feedback")).not.toBeInTheDocument();
  });

  it("automatically persists changes and reports that they were saved", async () => {
    render(<Preferences />);

    const autoCopy = await screen.findByRole("checkbox", {
      name: /Automatically copy captures to the clipboard/,
    });
    expect(autoCopy).toBeChecked();
    expect(screen.queryByRole("button", { name: "Close Preferences" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();

    fireEvent.click(autoCopy);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ auto_copy_to_clipboard: false }),
      });
    });
    expect(await screen.findByText("Changes saved")).toBeInTheDocument();
  });

  it("persists custom screenshot and recording countdown preferences", async () => {
    render(<Preferences />);

    const screenshotCountdown = await screen.findByRole("combobox", {
      name: "Screenshot countdown",
    });
    fireEvent.click(screenshotCountdown);
    fireEvent.click(await screen.findByRole("option", { name: "4 seconds" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ screenshot_countdown_seconds: 4 }),
      });
    });

    const recordingCountdown = screen.getByRole("combobox", { name: "Recording countdown" });
    fireEvent.click(recordingCountdown);
    fireEvent.click(await screen.findByRole("option", { name: "7 seconds" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({
          screenshot_countdown_seconds: 4,
          recording: expect.objectContaining({ countdown_seconds: 7 }),
        }),
      });
    });
  });

  it("can enable auto-start after target selection", async () => {
    render(<Preferences />);

    const autoStart = await screen.findByRole("checkbox", {
      name: /Start capture as soon as a target is selected/,
    });
    expect(autoStart).not.toBeChecked();
    fireEvent.click(autoStart);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ auto_start_on_selection: true }),
      });
    });
  });

  it("scrolls to and highlights auto-capture when opened with that target", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=preferences&target=auto-start-on-selection",
    );
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView,
    });

    render(<Preferences />);

    const autoStart = await screen.findByRole("checkbox", {
      name: /Start capture as soon as a target is selected/,
    });
    await waitFor(() => {
      expect(autoStart.closest("label")).toHaveClass("preference-target-highlight");
      expect(autoStart).toHaveFocus();
      expect(scrollIntoView).toHaveBeenCalledWith({ behavior: "smooth", block: "center" });
      expect(screen.getByRole("button", { name: "Capture" })).toHaveAttribute(
        "aria-current",
        "true",
      );
    });
  });

  it("retargets an existing Preferences window from the native event", async () => {
    let targetHandler: ((event: { payload: string }) => void) | undefined;
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "preferences-target") {
        targetHandler = handler as (event: { payload: string }) => void;
      }
      return () => undefined;
    });

    render(<Preferences />);
    const autoStart = await screen.findByRole("checkbox", {
      name: /Start capture as soon as a target is selected/,
    });
    targetHandler?.({ payload: "auto-start-on-selection" });

    await waitFor(() => {
      expect(autoStart.closest("label")).toHaveClass("preference-target-highlight");
      expect(autoStart).toHaveFocus();
    });
  });

  it("scrolls to and highlights recording-control visibility when opened with that target", async () => {
    window.history.replaceState(
      {},
      "",
      "/?view=preferences&target=include-recording-controls-in-captures",
    );
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView,
    });

    render(<Preferences />);

    const includeControls = await screen.findByRole("checkbox", {
      name: /Show recording controls in screenshots and recordings/,
    });
    await waitFor(() => {
      expect(includeControls.closest("label")).toHaveClass("preference-target-highlight");
      expect(includeControls).toHaveFocus();
      expect(scrollIntoView).toHaveBeenCalledWith({ behavior: "smooth", block: "center" });
      expect(screen.getByRole("button", { name: "Capture" })).toHaveAttribute(
        "aria-current",
        "true",
      );
    });
  });

  it("can disable quick-access mini previews", async () => {
    render(<Preferences />);

    const miniPreviews = await screen.findByRole("checkbox", {
      name: /Show mini previews after screenshots/,
    });
    expect(miniPreviews).toBeChecked();
    fireEvent.click(miniPreviews);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ show_mini_previews: false }),
      });
    });
    expect(screen.getByText(
      "Mini previews are off, so they won’t show in screenshots or recordings.",
    )).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Bottom left" })).toBeDisabled();
  });

  it("can move mini previews to another screen corner", async () => {
    render(<Preferences />);

    const topRight = await screen.findByRole("radio", { name: "Top right" });
    expect(screen.getByRole("radio", { name: "Bottom left" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    fireEvent.click(topRight);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ mini_preview_placement: "top_right" }),
      });
    });
    expect(topRight).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText("Top right")).toBeInTheDocument();
  });

  it("explains and updates mini preview visibility in screenshots and recordings", async () => {
    render(<Preferences />);

    const includePreviews = await screen.findByRole("checkbox", {
      name: /Show mini previews in screenshots and recordings/,
    });
    expect(includePreviews).not.toBeChecked();
    expect(includePreviews).toBeEnabled();
    expect(screen.getByText("Mini previews won’t show in screenshots or recordings."))
      .toBeInTheDocument();
    fireEvent.click(includePreviews);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ include_mini_previews_in_captures: true }),
      });
    });
    expect(screen.getByText(
      "Mini previews will show in screenshots and recordings. Turn this off to keep them out.",
    )).toBeInTheDocument();
  });

  it("explains and updates recording control visibility in screenshots and recordings", async () => {
    render(<Preferences />);

    const includeControls = await screen.findByRole("checkbox", {
      name: /Show recording controls in screenshots and recordings/,
    });
    expect(includeControls).not.toBeChecked();
    expect(screen.getByText("Recording controls won’t show in screenshots or recordings."))
      .toBeInTheDocument();
    expect(includeControls.closest("label")?.querySelector("strong")).toHaveTextContent("won’t");
    fireEvent.click(includeControls);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ include_recording_controls_in_captures: true }),
      });
    });
    expect(screen.getByText(
      "Recording controls will show in screenshots and recordings. Turn this off to keep them out.",
    )).toBeInTheDocument();
    expect(includeControls.closest("label")?.querySelector("strong")).toHaveTextContent("will");
  });

  it("automatically applies a newly recorded shortcut", async () => {
    render(<Preferences />);

    const recorder = await screen.findByRole("button", { name: "Window" });
    fireEvent.click(recorder);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_shortcut_capture_suppressed", {
        suppressed: true,
      });
    });
    fireEvent.keyDown(recorder, {
      code: "Digit0",
      key: ")",
      ctrlKey: true,
      shiftKey: true,
    });

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ window_shortcut: "Control+Shift+Digit0" }),
      });
    });
    expect(invoke).toHaveBeenCalledWith("set_shortcut_capture_suppressed", {
      suppressed: false,
    });
    expect(await screen.findByText("Changes saved")).toBeInTheDocument();
  });

  it("previews and persists a color theme", async () => {
    render(<Preferences />);

    const cobalt = await screen.findByRole("radio", { name: /Cobalt/ });
    expect(cobalt).not.toBeChecked();

    fireEvent.click(cobalt);

    expect(document.documentElement).toHaveAttribute("data-capture-theme", "cobalt");
    expect(cobalt).toBeChecked();
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ theme: "cobalt" }),
      });
    });
  });

  it("presents the expanded spectrum as compact theme choices", async () => {
    render(<Preferences />);

    expect(
      await within(await screen.findByRole("radiogroup", { name: "Color theme" }))
        .findAllByRole("radio"),
    ).toHaveLength(10);
    expect(screen.getByRole("radio", { name: /Violet/ })).toHaveAttribute(
      "data-capture-theme",
      "violet",
    );
    expect(screen.getByRole("radio", { name: /Cobalt/ })).toHaveAttribute(
      "data-capture-theme",
      "cobalt",
    );
    expect(screen.getByRole("radio", { name: /Mono/ })).toHaveAttribute(
      "title",
      "Vercel-like black and white",
    );
    expect(screen.getByRole("radio", { name: /Custom/ })).toHaveClass(
      "theme-option-custom",
    );
  });

  it("builds and persists a custom theme from editable colors", async () => {
    render(<Preferences />);

    fireEvent.click(await screen.findByRole("radio", { name: /Custom/ }));
    const editor = screen.getByRole("group", { name: "Custom theme colors" });
    expect(editor).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Accent color picker"), {
      target: { value: "#123456" },
    });
    const signalHex = screen.getByRole("textbox", {
      name: "Recording signal hex value",
    });
    fireEvent.change(signalHex, { target: { value: "#22AA55" } });
    fireEvent.blur(signalHex);

    expect(document.documentElement).toHaveAttribute("data-capture-theme", "custom");
    expect(document.documentElement.style.getPropertyValue("--theme-accent")).toBe("#123456");
    expect(document.documentElement.style.getPropertyValue("--theme-signal")).toBe("#22aa55");
    expect(document.documentElement.style.getPropertyValue("--theme-accent-hover")).not.toBe("");
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({
          theme: "custom",
          custom_theme: {
            accent: "#123456",
            signal: "#22aa55",
          },
        }),
      });
    });

    fireEvent.click(screen.getByRole("button", { name: "Reset colors" }));
    expect(document.documentElement.style.getPropertyValue("--theme-accent")).toBe("#32d3ff");
    expect(document.documentElement.style.getPropertyValue("--theme-signal")).toBe("#ff4fc3");
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({
          theme: "custom",
          custom_theme: {
            accent: "#32d3ff",
            signal: "#ff4fc3",
          },
        }),
      });
    });
  });

  it("presents the unified New Capture shortcut as the primary launcher", async () => {
    render(<Preferences />);

    const help = platformShortcutHelp(detectShortcutPlatform());
    const recorder = await screen.findByRole("button", { name: "New Capture" });
    expect(recorder).toHaveTextContent("Ctrl");
    expect(recorder).toHaveTextContent("Shift");
    expect(recorder).toHaveTextContent("Space");
    expect(screen.getByText((text) => text.includes(help.intro))).toBeInTheDocument();
    expect(screen.getByText(help.takeoverTitle)).toBeInTheDocument();
  });

  it("presents recording toggles as switch rows inside the Recording card", async () => {
    render(<Preferences />);

    const mono = await screen.findByRole("checkbox", {
      name: "Export recording audio in mono",
    });
    const showCursor = screen.getByRole("checkbox", {
      name: "Show cursor in recordings",
    });
    const showClicks = screen.getByRole("checkbox", {
      name: "Show clicks in recordings",
    });

    for (const toggle of [mono, showCursor, showClicks]) {
      expect(toggle.closest("label")).toHaveClass("check-row", "switch-row");
      expect(toggle.closest("section")).toHaveAttribute("id", "recording");
    }
  });

  it("moves between settings sections from the sidebar", async () => {
    render(<Preferences />);

    const shortcuts = await screen.findByRole("button", { name: "Shortcuts" });
    fireEvent.click(shortcuts);
    expect(shortcuts).toHaveAttribute("aria-current", "true");
  });

  it("persists the interface appearance", async () => {
    render(<Preferences />);

    fireEvent.click(await screen.findByRole("button", { name: "Light" }));

    expect(document.documentElement).toHaveAttribute("data-appearance", "light");
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ appearance: "light" }),
      });
    });
  });

  it("shows the installed version and offers a manual update check", async () => {
    render(<Preferences />);

    expect(await screen.findByText("Version 0.1.0")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "download from captur.es" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Check Now" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("check_for_updates"));
  });

  it("can hide release notes on update notices", async () => {
    render(<Preferences />);

    fireEvent.click(await screen.findByRole("button", { name: "Updates" }));
    const changelog = await screen.findByRole("checkbox", {
      name: /Show what’s new on update notices/,
    });
    expect(changelog).toBeChecked();
    fireEvent.click(changelog);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ show_update_changelog: false }),
      });
    });
  });

  it("does not label a manual Linux update with the AppImage size", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_settings") return settings;
      if (command === "get_update_status") {
        return {
          state: "available",
          current_version: "0.1.0",
          current_display_version: "0.1.0",
          version: "0.1.1",
          display_version: "0.1.1",
          notes: null,
          changelog: [],
          installable: false,
          manual_download_url: "https://captur.es/download",
          download_size: 12_582_912,
          will_close_open_captures: false,
        } satisfies UpdateStatus;
      }
      if (command === "set_shortcut_capture_suppressed") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<Preferences />);

    expect(await screen.findByText("Version 0.1.1 is available")).toBeInTheDocument();
    expect(screen.queryByText(/12\.6 MB/u)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "View release" })).toBeInTheDocument();
  });

  it("links a failed update to the website download page", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_settings") return settings;
      if (command === "get_update_status") {
        return {
          state: "error",
          current_version: "0.1.0",
          current_display_version: "0.1.0",
          message: "Could not install the update: Download request failed with status: 404 Not Found",
          retry_install: true,
        };
      }
      if (command === "set_shortcut_capture_suppressed") return undefined;
      if (command === "open_update_download_page") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<Preferences />);

    expect(await screen.findByText("Couldn’t check for updates")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "download from captur.es" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("open_update_download_page"));
  });

  it("can turn off freeze screen while capturing", async () => {
    render(<Preferences />);

    const freeze = await screen.findByRole("checkbox", {
      name: /Freeze screen when capturing/,
    });
    expect(freeze).toBeChecked();
    fireEvent.click(freeze);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ freeze_screen: false }),
      });
    });
  });

  it("can turn off the screenshot cursor", async () => {
    render(<Preferences />);

    const showCursor = await screen.findByRole("checkbox", {
      name: /Show cursor in screenshots/,
    });
    expect(showCursor).toBeChecked();
    fireEvent.click(showCursor);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ show_cursor_in_screenshots: false }),
      });
    });
  });

  it("persists default screenshot and recording file formats", async () => {
    render(<Preferences />);

    const screenshotFormat = await screen.findByRole("combobox", { name: "Screenshot format" });
    fireEvent.click(screenshotFormat);
    fireEvent.click(await screen.findByRole("option", { name: "JPEG" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ screenshot_format: "jpeg" }),
      });
    });

    const recordingFormat = screen.getByRole("combobox", { name: "Recording format" });
    fireEvent.click(recordingFormat);
    fireEvent.click(await screen.findByRole("option", { name: "WebM" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({
          screenshot_format: "jpeg",
          recording: expect.objectContaining({ video_format: "webm" }),
        }),
      });
    });
  });

  it("opens find with the platform shortcut and jumps to matching settings", async () => {
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView,
    });
    render(<Preferences />);
    await screen.findByRole("heading", { name: "Preferences" });
    expect(screen.queryByRole("searchbox", { name: "Find settings" })).not.toBeInTheDocument();

    fireEvent.keyDown(window, { key: "f", code: "KeyF", ctrlKey: true });
    const find = await screen.findByRole("searchbox", { name: "Find settings" });
    await waitFor(() => expect(find).toHaveFocus());

    fireEvent.change(find, { target: { value: "clipboard" } });
    expect(await screen.findByText("1 of 1")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", {
      name: /Automatically copy captures to the clipboard/,
    }).closest("label")).toHaveClass("preference-find-match", "preference-find-current");
    expect(scrollIntoView).toHaveBeenCalledWith({ behavior: "smooth", block: "center" });
  });

  it("cycles through matching settings and closes find with Escape", async () => {
    render(<Preferences />);
    await screen.findByRole("heading", { name: "Preferences" });

    fireEvent.keyDown(window, { key: "f", code: "KeyF", ctrlKey: true });
    const find = await screen.findByRole("searchbox", { name: "Find settings" });
    fireEvent.change(find, { target: { value: "cursor" } });
    expect(await screen.findByText("1 of 2")).toBeInTheDocument();

    fireEvent.keyDown(find, { key: "Enter", code: "Enter" });
    expect(screen.getByText("2 of 2")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Previous match" }));
    expect(screen.getByText("1 of 2")).toBeInTheDocument();

    fireEvent.change(find, { target: { value: "zzzz-not-a-setting" } });
    expect(await screen.findByText("No results")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next match" })).toBeDisabled();

    fireEvent.keyDown(window, { key: "Escape", code: "Escape" });
    expect(screen.queryByRole("searchbox", { name: "Find settings" })).not.toBeInTheDocument();
  });

  it("does not treat Enter on find-bar buttons as next-match", async () => {
    render(<Preferences />);
    await screen.findByRole("heading", { name: "Preferences" });

    fireEvent.keyDown(window, { key: "f", code: "KeyF", ctrlKey: true });
    const find = await screen.findByRole("searchbox", { name: "Find settings" });
    fireEvent.change(find, { target: { value: "cursor" } });
    expect(await screen.findByText("1 of 2")).toBeInTheDocument();

    const previous = screen.getByRole("button", { name: "Previous match" });
    previous.focus();
    fireEvent.keyDown(previous, { key: "Enter", code: "Enter" });
    expect(screen.getByText("1 of 2")).toBeInTheDocument();

    const close = screen.getByRole("button", { name: "Close find" });
    close.focus();
    fireEvent.keyDown(close, { key: "Enter", code: "Enter" });
    expect(screen.getByText("1 of 2")).toBeInTheDocument();
    expect(screen.getByRole("searchbox", { name: "Find settings" })).toBeInTheDocument();
  });

  it("uses Command+F on macOS and ignores Control+F there", async () => {
    window.history.replaceState({}, "", "/?platform=macos");
    render(<Preferences />);
    await screen.findByRole("heading", { name: "Preferences" });

    fireEvent.keyDown(window, { key: "f", code: "KeyF", ctrlKey: true });
    expect(screen.queryByRole("searchbox", { name: "Find settings" })).not.toBeInTheDocument();

    fireEvent.keyDown(window, { key: "f", code: "KeyF", metaKey: true });
    expect(await screen.findByRole("searchbox", { name: "Find settings" })).toBeInTheDocument();
  });

  it("does not steal the find chord while a shortcut is being recorded", async () => {
    render(<Preferences />);
    const recorder = await screen.findByRole("button", { name: "Window" });
    fireEvent.click(recorder);
    fireEvent.keyDown(recorder, { key: "f", code: "KeyF", ctrlKey: true });

    expect(screen.queryByRole("searchbox", { name: "Find settings" })).not.toBeInTheDocument();
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("update_settings", {
        settings: expect.objectContaining({ window_shortcut: "Control+KeyF" }),
      });
    });
  });
});
