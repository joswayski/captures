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
      if (command === "request_onboarding_microphone_permission") {
        currentState = {
          ...currentState,
          microphone_granted: true,
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
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("guides a new macOS user through screen access before enabling capture", async () => {
    render(<Onboarding />);

    expect(await screen.findByRole("heading", { name: "Required permissions" })).toBeInTheDocument();
    expect(screen.getByText(/Captures needs screen access to work/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start capturing" })).toBeDisabled();
    expect(screen.getByRole("navigation", { name: "Setup progress" })).toBeInTheDocument();
    expect(screen.queryByText("One place for access")).not.toBeInTheDocument();
    expect(screen.queryByText("Desktop audio")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Use by default" })).not.toBeInTheDocument();
    expect(screen.queryByText("Welcome")).not.toBeInTheDocument();
    expect(screen.queryByText("First-run setup")).not.toBeInTheDocument();
    expect(screen.queryByText("1 step left")).not.toBeInTheDocument();
    expect(screen.queryByText("Screenshot")).not.toBeInTheDocument();
    expect(screen.queryByText(/Setup for/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Your work stays yours/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Everything ready before your first capture/)).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Captures")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Allow access" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("request_onboarding_screen_permission");
    });
    expect(await screen.findByText("Restart required")).toBeInTheDocument();
    expect(screen.getByText(/Turn the switch on next to this copy of Captures/)).toBeInTheDocument();
    expect(screen.getByText(/A local build is a different row from a downloaded app/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open Settings" })).toBeInTheDocument();
    const restart = screen.getByRole("button", { name: "Restart Captures" });
    expect(restart).toBeInTheDocument();
    expect(restart.closest("section")).not.toBeNull();
    expect(screen.queryByRole("button", { name: "Check again" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Start capturing" })).not.toBeInTheDocument();
  });

  it("picks up Screen Recording after the user enables it in Settings", async () => {
    render(<Onboarding />);

    fireEvent.click(await screen.findByRole("button", { name: "Allow access" }));
    expect(await screen.findByText("Restart required")).toBeInTheDocument();

    currentState = {
      ...currentState,
      screen_recording_granted: true,
    };
    window.dispatchEvent(new Event("focus"));

    expect(await screen.findByText("Granted")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start capturing" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Restart Captures" })).not.toBeInTheDocument();
  });

  it("tells the user the switch is still off after a restart without access", async () => {
    currentState = {
      ...macNeedsPermission,
      screen_recording_can_request: false,
      screen_recording_requested_this_launch: false,
    };
    render(<Onboarding />);

    expect(await screen.findByText("Still off")).toBeInTheDocument();
    expect(screen.getByText(/The switch for this copy of Captures is still off/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open Settings" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start capturing" })).toBeDisabled();
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
    expect(screen.getByText("Granted")).toBeInTheDocument();
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

    expect(await screen.findByRole("heading", { name: "You’re ready to capture" })).toBeInTheDocument();
    expect(screen.getByText("Windows provides screen capture access without a separate permission prompt. Secure and protected windows remain private.")).toBeInTheDocument();
    expect(screen.getByText("Ready")).toBeInTheDocument();
    expect(screen.queryByText("Desktop audio")).not.toBeInTheDocument();
    expect(screen.queryByText("Microphone")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Allow access" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start capturing" })).toBeEnabled();
  });

  it("lets a new macOS user grant the microphone without turning it on by default", async () => {
    render(<Onboarding />);

    expect(await screen.findByText("Microphone")).toBeInTheDocument();
    expect(screen.getByText("Optional")).toHaveClass("optional");
    fireEvent.click(screen.getByRole("button", { name: "Allow microphone" }));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("request_onboarding_microphone_permission");
    });
    expect(await screen.findByText("Granted")).toBeInTheDocument();
    expect(screen.getByText(/Turn the microphone on when you start a recording/)).toBeInTheDocument();
    expect(screen.queryByText("On by default")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("set_onboarding_desktop_audio", expect.anything());
    expect(invoke).not.toHaveBeenCalledWith("set_onboarding_microphone", expect.anything());
  });

  it("restarts after returning from Settings when access is still missing", async () => {
    const now = Date.now();
    const nowSpy = vi.spyOn(Date, "now").mockReturnValue(now);

    render(<Onboarding />);
    fireEvent.click(await screen.findByRole("button", { name: "Allow access" }));
    expect(await screen.findByRole("button", { name: "Restart Captures" })).toBeInTheDocument();

    window.dispatchEvent(new Event("blur"));
    nowSpy.mockReturnValue(now + 3_000);
    window.dispatchEvent(new Event("focus"));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("restart_captures_for_permissions");
    });
    nowSpy.mockRestore();
  });
});
