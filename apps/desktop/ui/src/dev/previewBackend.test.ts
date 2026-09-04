import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { clearMocks } from "@tauri-apps/api/mocks";

import { installPreviewBackend } from "./previewBackend";
import type { AppSettings, RecordingSelectionSession } from "../types";

describe("previewBackend capture display switching", () => {
  beforeEach(() => {
    installPreviewBackend();
  });

  afterEach(() => {
    clearMocks();
  });

  it("returns an updated selection instead of undefined when switching displays", async () => {
    const current = await invoke<RecordingSelectionSession>("get_recording_selection");
    expect(current.display.id).toBe("display-1");
    expect(current.windows.length).toBeGreaterThan(0);

    const next = await invoke<RecordingSelectionSession>("select_capture_display", {
      selectionId: current.id,
      displayId: "display-2",
    });

    expect(next.id).toBe(current.id);
    expect(next.display.id).toBe("display-2");
    expect(next.display.name).toBe("Studio Display");
    expect(next.display.width).toBe(1920);
    expect(next.display.height).toBe(1080);
    expect(next.snapshot_url).not.toBe(current.snapshot_url);
    expect(next.windows.every((window) => window.display_id === "display-2")).toBe(true);

    const stored = await invoke<RecordingSelectionSession>("get_recording_selection");
    expect(stored.display.id).toBe("display-2");
  });

  it("keeps preference changes instead of reverting to the sample settings", async () => {
    const current = await invoke<AppSettings>("get_settings");
    expect(current.mini_preview_placement).toBe("bottom_left");

    const saved = await invoke<AppSettings>("update_settings", {
      settings: { ...current, mini_preview_placement: "top_right" },
    });
    expect(saved.mini_preview_placement).toBe("top_right");
    expect((await invoke<AppSettings>("get_settings")).mini_preview_placement).toBe("top_right");

    await invoke("update_settings", { settings: current });
  });

  it("rejects an unknown display without returning undefined", async () => {
    const current = await invoke<RecordingSelectionSession>("get_recording_selection");
    await expect(invoke("select_capture_display", {
      selectionId: current.id,
      displayId: "missing",
    })).rejects.toThrow(/display is unavailable/);
  });
});
