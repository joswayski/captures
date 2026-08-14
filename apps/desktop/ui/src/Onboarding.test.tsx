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
    expect(screen.getByRole("button", { name: "Start capturing" })).toBeDisabled();
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
    };
    render(<Onboarding />);

    expect(await screen.findByText("Windows provides screen capture access without a separate permission prompt. Secure and protected windows remain private.")).toBeInTheDocument();
    expect(screen.getByText("Built in")).toBeInTheDocument();
    expect(screen.getByText("Ready")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Allow access" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start capturing" })).toBeEnabled();
  });
});
