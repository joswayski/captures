import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { Onboarding } from "./Onboarding";
import type { OnboardingState } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

const macNeedsPermission: OnboardingState = {
  platform: "macos",
  screen_recording_required: true,
  screen_recording_granted: false,
  screen_recording_can_request: true,
  screen_recording_requested_this_launch: false,
  capture_system_audio: false,
  microphone_enabled: false,
  microphone_granted: false,
  microphone_can_request: true,
};

describe("Onboarding", () => {
  let currentState: OnboardingState;

  beforeEach(() => {
    currentState = { ...macNeedsPermission };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_onboarding_state") return currentState;
      if (command === "request_onboarding_screen_permission") {
        currentState = {
          ...currentState,
          screen_recording_can_request: false,
          screen_recording_requested_this_launch: true,
        };
        return currentState;
      }
      if (command === "set_onboarding_desktop_audio") {
        currentState = { ...currentState, capture_system_audio: true };
        return currentState;
      }
      if (command === "set_onboarding_microphone") {
        currentState = {
          ...currentState,
          microphone_enabled: currentState.microphone_granted,
        };
        return currentState;
      }
      if (command === "request_onboarding_microphone_permission") {
        currentState = {
          ...currentState,
          microphone_granted: true,
          microphone_enabled: true,
          microphone_can_request: false,
        };
        return currentState;
      }
      if (command === "restart_captures_for_permissions") return undefined;
      if (command === "complete_onboarding") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("guides a new macOS user through screen access before enabling capture", async () => {
    render(<Onboarding />);

    expect(await screen.findByRole("heading", { name: "One place for access" })).toBeInTheDocument();
    expect(screen.getByText("Needs approval")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start capturing" })).toBeDisabled();
    expect(screen.queryByText("Welcome")).not.toBeInTheDocument();
    expect(screen.queryByText("First-run setup")).not.toBeInTheDocument();
    expect(screen.queryByText("1 step left")).not.toBeInTheDocument();
    expect(screen.queryByText("Screenshot")).not.toBeInTheDocument();
    expect(screen.queryByText(/Setup for/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Your work stays yours/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Everything ready before your first capture/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Allow access" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("request_onboarding_screen_permission");
    });
    expect(await screen.findByText("Waiting for macOS")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open Settings" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restart Captures" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Check again" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start capturing" })).toBeDisabled();
  });

  it("picks up Screen Recording after the user enables it in Settings", async () => {
    render(<Onboarding />);

    fireEvent.click(await screen.findByRole("button", { name: "Allow access" }));
    expect(await screen.findByText("Waiting for macOS")).toBeInTheDocument();

    currentState = {
      ...currentState,
      screen_recording_granted: true,
    };
    window.dispatchEvent(new Event("focus"));

    expect(await screen.findByText("Allowed")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start capturing" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Restart Captures" })).not.toBeInTheDocument();
  });

  it("finishes setup once required macOS access is available", async () => {
    currentState = {
      ...macNeedsPermission,
      screen_recording_granted: true,
      screen_recording_can_request: false,
    };
    render(<Onboarding />);

    const start = await screen.findByRole("button", { name: "Start capturing" });
    expect(start).toBeEnabled();
    expect(screen.getByText("Allowed")).toBeInTheDocument();
    fireEvent.click(start);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("complete_onboarding");
    });
  });

  it("does not invent an up-front permission step on Windows", async () => {
    currentState = {
      platform: "windows",
      screen_recording_required: false,
      screen_recording_granted: true,
      screen_recording_can_request: false,
      screen_recording_requested_this_launch: false,
      capture_system_audio: false,
      microphone_enabled: false,
      microphone_granted: true,
      microphone_can_request: false,
    };
    render(<Onboarding />);

    expect(await screen.findByText("Windows provides screen capture access without a separate permission prompt. Secure and protected windows remain private.")).toBeInTheDocument();
    expect(screen.getByText("Built in")).toBeInTheDocument();
    expect(screen.getByText("Ready")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Allow access" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start capturing" })).toBeEnabled();
  });

  it("lets a new user enable desktop audio and the microphone from first run", async () => {
    render(<Onboarding />);

    fireEvent.click(await screen.findByRole("button", { name: "Use by default" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("set_onboarding_desktop_audio", { enabled: true });
    });
    expect(await screen.findByText("On by default")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Allow microphone" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("request_onboarding_microphone_permission");
    });
    expect(screen.getAllByText("On by default")).toHaveLength(2);
  });
});
