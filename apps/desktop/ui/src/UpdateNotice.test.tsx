import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { UpdateNotice } from "./App";
import type { UpdateStatus } from "./types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(async () => () => undefined),
}));

const available: UpdateStatus = {
  state: "available",
  current_version: "2026.7.1901",
  current_display_version: "2026.07.19.1",
  version: "2026.7.1902",
  display_version: "2026.07.19.2",
  notes: "> [!WARNING]\n> This Preview is experimental.\n\n## What's Changed\n* Adds automatic releases by @joswayski in https://github.com/joswayski/captures/pull/1\n\n**Full Changelog**: https://github.com/joswayski/captures/compare/old...new",
  installable: true,
  manual_download_url: null,
};

describe("UpdateNotice", () => {
  afterEach(() => vi.clearAllMocks());

  it("offers an explicit install and restart for an available update", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") return available;
      if (command === "install_update") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);

    expect(await screen.findByRole("dialog", {
      name: "A new Captures Preview is ready",
    })).toBeInTheDocument();
    expect(screen.getByLabelText(
      "Updating Captures from version 2026.07.19.1 to 2026.07.19.2",
    )).toBeInTheDocument();
    expect(screen.getByText("Adds automatic releases")).toBeInTheDocument();
    expect(screen.queryByText(/experimental/u)).not.toBeInTheDocument();
    expect(screen.queryByText(/Full Changelog/u)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Install & Restart" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("install_update"));
  });

  it("shows download progress while installation is running", async () => {
    vi.mocked(invoke).mockResolvedValue({
      state: "downloading",
      current_version: "2026.7.1901",
      current_display_version: "2026.07.19.1",
      version: "2026.7.1902",
      display_version: "2026.07.19.2",
      downloaded: 25,
      total: 100,
    } satisfies UpdateStatus);

    render(<UpdateNotice />);

    expect(await screen.findByText("Downloading… 25%")).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Downloading update" })).toHaveAttribute(
      "aria-valuenow",
      "25",
    );
    expect(screen.getByRole("button", { name: "Later" })).toBeDisabled();
  });

  it("keeps the available state useful when release notes are missing", async () => {
    vi.mocked(invoke).mockResolvedValue({ ...available, notes: null } satisfies UpdateStatus);

    render(<UpdateNotice />);

    expect(await screen.findByText("The latest Captures improvements")).toBeInTheDocument();
    expect(screen.getByText(/Release notes aren’t available/u)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Install & Restart" })).toBeEnabled();
  });

  it("shows the restart countdown after installation", async () => {
    vi.mocked(invoke).mockResolvedValue({
      state: "restarting",
      current_version: "2026.7.1901",
      current_display_version: "2026.07.19.1",
      version: "2026.7.1902",
      display_version: "2026.07.19.2",
      seconds_remaining: 3,
    } satisfies UpdateStatus);

    render(<UpdateNotice />);

    expect(await screen.findByText("Update installed successfully")).toBeInTheDocument();
    expect(screen.getByText("Restarting Captures in 3…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Later" })).toBeDisabled();
  });

  it("allows a failed check to be retried", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") {
        return {
          state: "error",
          current_version: "2026.7.1901",
          current_display_version: "2026.07.19.1",
          message: "GitHub is unavailable",
        } satisfies UpdateStatus;
      }
      if (command === "check_for_updates") return available;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);

    expect(await screen.findByRole("alert")).toHaveTextContent("GitHub is unavailable");
    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("check_for_updates"));
  });
});
