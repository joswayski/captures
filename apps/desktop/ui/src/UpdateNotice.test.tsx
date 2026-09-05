import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";

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
  download_size: 12_582_912,
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
  afterEach(() => {
    vi.clearAllMocks();
    vi.mocked(listen).mockImplementation(async () => () => undefined);
  });

  it("presents an available update without repeating metadata", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") return available;
      if (command === "install_update") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);

    expect(await screen.findByRole("dialog", {
      name: "Update available",
    })).toBeInTheDocument();
    expect(screen.getByText("Version 2026.07.19.2 · 12.6 MB")).toBeInTheDocument();
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
    expect(screen.getByRole("button", { name: "Update now" })).toHaveFocus();
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("install_update"));
  });

  it("hides AppImage size metadata for manual Linux updates", async () => {
    vi.mocked(invoke).mockResolvedValue({
      ...available,
      installable: false,
      manual_download_url: "https://captur.es/download",
    } satisfies UpdateStatus);

    render(<UpdateNotice />);

    expect(await screen.findByText("Version 2026.07.19.2")).toBeInTheDocument();
    expect(screen.queryByText(/12\.6 MB/u)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "View release" })).toBeInTheDocument();
  });

  it("dismisses Later through a native command instead of Window.hide", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") return available;
      if (command === "dismiss_update_notice") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);
    fireEvent.click(await screen.findByRole("button", { name: "Later" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("dismiss_update_notice"));
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
    expect(screen.getByText("Version 2026.08.27.5 · 12.6 MB")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Later" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("dismiss_update_notice"));
  });

  it("shows download progress while installation is running", async () => {
    vi.mocked(invoke).mockResolvedValue({
      state: "downloading",
      current_version: "2026.7.1901",
      current_display_version: "2026.07.19.1",
      version: "2026.7.1902",
      display_version: "2026.07.19.2",
      downloaded: 7_340_032,
      total: 12_582_912,
    } satisfies UpdateStatus);

    render(<UpdateNotice />);

    expect(await screen.findByText("58%")).toBeInTheDocument();
    expect(screen.getByText("7.3 MB / 12.6 MB")).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Downloading update" })).toHaveAttribute(
      "aria-valuenow",
      "58",
    );
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Updating Captures" })).toHaveFocus();
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

    expect(await screen.findByText("Updated")).toBeInTheDocument();
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
    expect(screen.getByRole("button", { name: "download from captur.es" })).toBeInTheDocument();
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
    expect(screen.getByRole("dialog", { name: "Update available" })).toBeInTheDocument();
    expect(updateNow).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Update now" }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("install_update"));
    expect(screen.queryByText("Update failed")).not.toBeInTheDocument();
  });

  it("clears a stale open-captures warning when native state is refreshed", async () => {
    let updateStatusChanged: ((event: { payload: UpdateStatus }) => void) | undefined;
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "update-status-changed") {
        updateStatusChanged = handler as (event: { payload: UpdateStatus }) => void;
      }
      return () => undefined;
    });
    vi.mocked(invoke).mockResolvedValue({
      ...available,
      will_close_open_captures: true,
    } satisfies UpdateStatus);

    render(<UpdateNotice />);

    expect(await screen.findByText("Open captures will close. Unsaved edits are kept as drafts."))
      .toBeInTheDocument();
    await waitFor(() => expect(updateStatusChanged).toBeDefined());
    await act(async () => {
      updateStatusChanged?.({
        payload: { ...available, will_close_open_captures: false },
      });
    });

    await waitFor(() => {
      expect(screen.queryByText("Open captures will close. Unsaved edits are kept as drafts."))
        .not.toBeInTheDocument();
    });
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
      if (command === "open_update_download_page") return undefined;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);

    expect(await screen.findByText("Update failed")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("403 Forbidden");
    fireEvent.click(screen.getByRole("button", { name: "download from captur.es" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("open_update_download_page"));
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
    expect(await screen.findByRole("dialog", { name: "Update available" })).toBeInTheDocument();
    const notice = container.querySelector(".tray-notice");
    expect(notice).toHaveAttribute("data-caret", "top");
    expect((notice as HTMLElement | null)?.style.getPropertyValue("--tray-caret-x")).toBe("220px");
    expect(container.querySelector(".tray-notice-caret")).toBeInTheDocument();
    window.history.replaceState({}, "", "/");
  });

  it("focuses Update now instead of Later when an update is available", async () => {
    vi.mocked(invoke).mockResolvedValue(available);

    render(<UpdateNotice />);

    expect(await screen.findByRole("button", { name: "Update now" })).toHaveFocus();
    expect(screen.queryByRole("button", { name: "download from captur.es" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Later" })).not.toHaveFocus();
  });

  it("does not let Escape dismiss an in-progress installation", async () => {
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
    expect(await screen.findByRole("dialog", { name: "Updating Captures" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByRole("dialog", { name: "Updating Captures" })).toBeInTheDocument();
  });

  it("blocks dismiss as soon as Update now is clicked, before download status arrives", async () => {
    let finishInstall: (value?: undefined) => void = () => undefined;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") return available;
      if (command === "install_update") {
        await new Promise<undefined>((resolve) => {
          finishInstall = resolve;
        });
        return undefined;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);
    fireEvent.click(await screen.findByRole("button", { name: "Update now" }));

    expect(await screen.findByRole("button", { name: "Later" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Update now" })).toBeDisabled();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByRole("dialog", { name: "Update available" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Later" })).toBeDisabled();

    finishInstall();
    await waitFor(() => expect(screen.getByRole("button", { name: "Later" })).toBeEnabled());
  });

  it("can hide release notes from the update notice", async () => {
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_update_status") return available;
      if (command === "get_settings") return { show_update_changelog: true };
      if (command === "update_settings") return (args as { settings: { show_update_changelog: boolean } }).settings;
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);

    expect(await screen.findByRole("region", { name: "What's new" })).toBeInTheDocument();
    expect(screen.getByText("Adds automatic releases")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Hide" }));

    await waitFor(() => {
      expect(screen.queryByRole("region", { name: "What's new" })).not.toBeInTheDocument();
    });
    expect(screen.queryByText("Adds automatic releases")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "What’s new" })).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("update_settings", {
      settings: expect.objectContaining({ show_update_changelog: false }),
    });
  });

  it("stays compact when release notes are turned off", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_update_status") return stacked;
      if (command === "get_settings") return { show_update_changelog: false };
      throw new Error(`unexpected command: ${command}`);
    });

    render(<UpdateNotice />);

    expect(await screen.findByRole("dialog", { name: "Update available" })).toBeInTheDocument();
    expect(screen.queryByText("This update includes all of the following changes:")).not.toBeInTheDocument();
    expect(screen.queryByText("Fix post-update launch notice position on macOS")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "What’s new" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Update now" })).toBeInTheDocument();
  });
});
