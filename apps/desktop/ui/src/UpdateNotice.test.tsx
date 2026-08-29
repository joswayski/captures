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
  notes: "> [!WARNING]\n> This Preview is experimental.\n\n## What's Changed\n* Adds automatic releases by @joswayski in https://github.com/joswayski/captures/pull/1\n* @devin-ai-integration[bot] made their first contribution in https://github.com/joswayski/captures/pull/1\n\n**Full Changelog**: https://github.com/joswayski/captures/compare/old...new",
  changelog: [],
  installable: true,
  manual_download_url: null,
  will_close_open_captures: false,
};

const stacked: UpdateStatus = {
  ...available,
  version: "2026.8.2705",
  display_version: "2026.08.27.5",
  notes: "* Fix the latest Preview only",
  changelog: [
    {
      version: "2026.8.2705",
      display_version: "2026.08.27.5",
      notes: "> [!WARNING]\n> Experimental.\n\n## What's Changed\n* Fix post-update launch notice position on macOS by @joswayski in https://github.com/example/pull/265",
    },
    {
      version: "2026.8.2704",
      display_version: "2026.08.27.4",
      notes: "* Fix capture menu display switching and the Record CTA by @joswayski in https://github.com/example/pull/263",
    },
    {
      version: "2026.8.2703",
      display_version: "2026.08.27.3",
      notes: "* Redesign the desktop UI around one design system by @joswayski in https://github.com/example/pull/262",
    },
  ],
};

describe("UpdateNotice", () => {
  afterEach(() => vi.clearAllMocks());

  it("presents an available update without repeating metadata", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") return available;
      if (command === "install_update") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);

    expect(await screen.findByRole("dialog", {
      name: "An update is available",
    })).toBeInTheDocument();
    expect(screen.getAllByText("Version 2026.07.19.2")).toHaveLength(1);
    expect(screen.queryByText("Open captures will close. Unsaved edits are kept as drafts."))
      .not.toBeInTheDocument();
    expect(screen.getByText("Adds automatic releases")).toBeInTheDocument();
    expect(screen.queryByText(/first contribution/iu)).not.toBeInTheDocument();
    expect(screen.queryByText(/experimental/u)).not.toBeInTheDocument();
    expect(screen.queryByText(/Full Changelog/u)).not.toBeInTheDocument();
    expect(screen.queryByText(/highlights/u)).not.toBeInTheDocument();
    expect(screen.queryByText(/Captures Preview/u)).not.toBeInTheDocument();
    expect(screen.queryByText(/Signed Preview/u)).not.toBeInTheDocument();
    expect(screen.queryByText("2026.07.19.1")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("install_update"));
  });

  it("groups skipped Preview notes by version", async () => {
    vi.mocked(invoke).mockResolvedValue(stacked);

    render(<UpdateNotice />);

    expect(await screen.findByText("This update includes all of the following changes:")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "2026.08.27.5" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "2026.08.27.4" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "2026.08.27.3" })).toBeInTheDocument();
    expect(screen.getByText("Fix post-update launch notice position on macOS")).toBeInTheDocument();
    expect(screen.getByText("Fix capture menu display switching and the Record CTA")).toBeInTheDocument();
    expect(screen.getByText("Redesign the desktop UI around one design system")).toBeInTheDocument();
    expect(screen.queryByText("Fix the latest Preview only")).not.toBeInTheDocument();
    expect(screen.getAllByText("Version 2026.08.27.5")).toHaveLength(1);
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

    expect(await screen.findByText("25%")).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Downloading update" })).toHaveAttribute(
      "aria-valuenow",
      "25",
    );
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("keeps the available state useful when release notes are missing", async () => {
    vi.mocked(invoke).mockResolvedValue({ ...available, notes: null } satisfies UpdateStatus);

    render(<UpdateNotice />);

    expect(await screen.findByText("Release notes aren’t available for this update.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Update now" })).toBeEnabled();
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

    expect(await screen.findByText("Update complete")).toBeInTheDocument();
    expect(screen.getByText("Reopening in 3 seconds…")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("allows a failed check to be retried", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") {
        return {
          state: "error",
          current_version: "2026.7.1901",
          current_display_version: "2026.07.19.1",
          message: "GitHub is unavailable",
          retry_install: false,
        } satisfies UpdateStatus;
      }
      if (command === "check_for_updates") return available;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);

    expect(await screen.findByRole("alert")).toHaveTextContent("GitHub is unavailable");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("check_for_updates"));
  });

  it("warns that open captures will close and still installs", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") {
        return { ...available, will_close_open_captures: true } satisfies UpdateStatus;
      }
      if (command === "install_update") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);

    const warning = await screen.findByText("Open captures will close. Unsaved edits are kept as drafts.");
    const notes = screen.getByRole("region", { name: "What's new" });
    const updateNow = screen.getByRole("button", { name: "Update now" });
    expect(warning.compareDocumentPosition(notes) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy();
    expect(warning.compareDocumentPosition(updateNow) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(warning.querySelector("svg")).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "An update is available" })).toBeInTheDocument();
    expect(updateNow).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("install_update"));
    expect(screen.queryByText("Update failed")).not.toBeInTheDocument();
  });

  it("retries installation when an available update was blocked", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") return available;
      if (command === "install_update") {
        throw "Finish or cancel the active recording before installing the update.";
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);
    fireEvent.click(await screen.findByRole("button", { name: "Update now" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Finish or cancel the active recording before installing the update.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));

    await waitFor(() => {
      expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === "install_update"))
        .toHaveLength(2);
    });
    expect(invoke).not.toHaveBeenCalledWith("check_for_updates");
  });

  it("retries a failed download instead of checking for updates again", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") {
        return {
          state: "error",
          current_version: "2026.7.1901",
          current_display_version: "2026.07.19.1",
          message: "Could not install the update: Download request failed with status: 403 Forbidden",
          retry_install: true,
        } satisfies UpdateStatus;
      }
      if (command === "install_update") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);

    expect(await screen.findByText("Update failed")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("403 Forbidden");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("install_update"));
    expect(invoke).not.toHaveBeenCalledWith("check_for_updates");
  });

  it("renders a tray caret when placement is provided", async () => {
    window.history.replaceState({}, "", "/?view=update&caret=top&caret_x=220");
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") return available;
      throw new Error(`unexpected command: ${command}`);
    });

    const { container } = render(<UpdateNotice />);
    expect(await screen.findByRole("dialog", { name: "An update is available" })).toBeInTheDocument();
    const notice = container.querySelector(".tray-notice");
    expect(notice).toHaveAttribute("data-caret", "top");
    expect((notice as HTMLElement | null)?.style.getPropertyValue("--tray-caret-x")).toBe("220px");
    expect(container.querySelector(".tray-notice-caret")).toBeInTheDocument();
    window.history.replaceState({}, "", "/");
  });
});
