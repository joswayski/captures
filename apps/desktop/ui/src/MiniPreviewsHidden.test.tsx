import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  MiniPreviewsHiddenChip,
} from "./App";
import {
  miniPreviewsHiddenLabel,
  takeMiniPreviewRestorePending,
} from "./lib/miniPreviewsHidden";
import type { CaptureArtifact } from "./types";

const miniPreviewStyles = readFileSync(
  resolve(process.cwd(), "ui/src/styles/mini-preview.css"),
  "utf8",
);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}));

const artifacts: CaptureArtifact[] = [
  {
    id: "capture-1",
    path: null,
    preview_url: "captures-capture://artifact/capture-1",
    full_url: "captures-capture://artifact-full/capture-1",
    width: 1_440,
    height: 900,
    size_bytes: 250_000,
    created_at: "2026-07-19T18:00:00Z",
    mode: "region",
    history_saved: true,
    clipboard_copy_status: "copied",
  },
  {
    id: "capture-2",
    path: null,
    preview_url: "captures-capture://artifact/capture-2",
    full_url: "captures-capture://artifact-full/capture-2",
    width: 800,
    height: 600,
    size_bytes: 80_000,
    created_at: "2026-07-19T18:01:00Z",
    mode: "region",
    history_saved: true,
    clipboard_copy_status: "skipped",
  },
];

describe("miniPreviewsHiddenLabel", () => {
  it("pluralizes parked preview counts", () => {
    expect(miniPreviewsHiddenLabel(0)).toBe("Previews");
    expect(miniPreviewsHiddenLabel(1)).toBe("1 preview");
    expect(miniPreviewsHiddenLabel(3)).toBe("3 previews");
  });
});

describe("MiniPreviewsHiddenChip", () => {
  beforeEach(() => {
    window.history.replaceState({}, "", "/?view=mini-previews-hidden&count=2");
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_artifacts") return artifacts;
      return undefined;
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    takeMiniPreviewRestorePending();
    vi.clearAllMocks();
  });

  it("keeps the full restore chip pointer-stable with visible hover feedback", () => {
    const baseRule = miniPreviewStyles.match(
      /\.mini-previews-hidden\s*\{([\s\S]*?)\n\}/,
    );
    const childRule = miniPreviewStyles.match(
      /\.mini-previews-hidden > \*\s*\{([\s\S]*?)\n\}/,
    );
    const hoverRule = miniPreviewStyles.match(
      /\.mini-previews-hidden:hover,[\s\S]*?\{([\s\S]*?)\n\}/,
    );

    expect(baseRule?.[1]).toMatch(/cursor:\s*pointer !important/);
    expect(baseRule?.[1]).toMatch(/background:\s*var\(--glass-strong-solid\)/);
    expect(baseRule?.[1]).not.toMatch(/backdrop-filter/);
    expect(childRule?.[1]).toMatch(/pointer-events:\s*none/);
    expect(hoverRule?.[1]).toMatch(/border-color:\s*var\(--glass-border-strong\)/);
    expect(hoverRule?.[1]).not.toMatch(/theme-accent-rgb/);
  });

  it("keeps the restore chip shadow inside its native window gutter", () => {
    const baseRule = miniPreviewStyles.match(
      /\.mini-previews-hidden\s*\{([\s\S]*?)\n\}/,
    );
    const rootAnimation = miniPreviewStyles.match(
      /@keyframes mini-previews-hidden-in\s*\{([\s\S]*?)\n\}/,
    );
    const restoringRule = miniPreviewStyles.match(
      /\.mini-previews-hidden-restoring\s*\{([\s\S]*?)\n\}/,
    );

    expect(baseRule?.[1]).toMatch(/width:\s*48px/);
    expect(baseRule?.[1]).toMatch(/height:\s*48px/);
    expect(baseRule?.[1]).toMatch(/left:\s*8px/);
    expect(baseRule?.[1]).toMatch(/bottom:\s*8px/);
    expect(baseRule?.[1]).toMatch(
      /0 var\(--s-1\) var\(--s-3\) rgba\(0, 0, 0, 0\.34\)/,
    );
    expect(baseRule?.[1]).not.toMatch(/var\(--glass-shadow\)/);
    expect(rootAnimation?.[1]).not.toMatch(/transform:/);
    expect(restoringRule?.[1]).not.toMatch(/width:/);
  });

  it("opens the 3D folder without resizing its glass control", () => {
    const collapseRule = miniPreviewStyles.match(
      /\.thumbnail-collapse\s*\{([\s\S]*?)\n\}/,
    );

    expect(collapseRule?.[1]).toMatch(/width:\s*48px/);
    expect(collapseRule?.[1]).toMatch(/height:\s*48px/);
    expect(miniPreviewStyles).toMatch(/clip-path:\s*polygon/);
    expect(miniPreviewStyles).toMatch(/data-pose="parked"/);
    expect(miniPreviewStyles).not.toMatch(
      /\.thumbnail-collapse\.thumbnail-collapse-(?:collapsing|parked|restoring)[^{]*\{[^}]*width:/s,
    );
    expect(miniPreviewStyles).toMatch(
      /\.thumbnail-collapse\.thumbnail-collapse-collapsing,[\s\S]*?\.thumbnail-collapse\.thumbnail-collapse-restoring\s*\{[^}]*background:\s*transparent/s,
    );
    expect(miniPreviewStyles).toMatch(/transform-style:\s*preserve-3d/);
    expect(miniPreviewStyles).toMatch(/rotateX\(-62deg\)/);
    expect(miniPreviewStyles).toMatch(/\.mini-preview-folder-flap/);
    expect(miniPreviewStyles).toMatch(/\.mini-preview-folder-pocket/);
    expect(miniPreviewStyles).toMatch(/\.mini-preview-folder-tab/);
  });

  it("hands the folder surface off to real clipped dust chips", () => {
    const exitRule = miniPreviewStyles.match(
      /\.thumbnail-collapse\.thumbnail-collapse-leaving\s*\{([\s\S]*?)\n\}/,
    );

    expect(exitRule?.[1]).toMatch(
      /thumbnail-folder-source-fade 0\.18s var\(--mini-preview-folder-dust-lead\)/,
    );
    expect(exitRule?.[1]).not.toMatch(/mask-image|radial-gradient/);
    expect(miniPreviewStyles).toMatch(/\.thumbnail-collapse-dust-layer/);
    expect(miniPreviewStyles).toMatch(/\.thumbnail-collapse-dust-surface/);
    expect(miniPreviewStyles).toMatch(
      /\.thumbnail-collapse-dust-layer \.thumbnail-collapse-dust-chip\s*\{[^}]*filter:\s*none/,
    );
    expect(miniPreviewStyles).toMatch(
      /\.thumbnail-collapse-dust-layer \.thumbnail-collapse-dust-chip\s*\{[^}]*opacity:\s*0/,
    );
    expect(miniPreviewStyles).toMatch(
      /\.thumbnail-collapse-dust-surface\s*\{[^}]*background:\s*transparent/,
    );
    expect(miniPreviewStyles).not.toMatch(
      /\.thumbnail-collapse-dust-surface\s*\{[^}]*background:\s*var\(--glass-strong-solid\)/,
    );
    expect(miniPreviewStyles).toMatch(/--mini-preview-folder-dust-lead:\s*370ms/);
  });

  it("restores the stack from the parked folder without shrinking first", async () => {
    render(<MiniPreviewsHiddenChip />);

    const chip = screen.getByRole("button", { name: "Show 2 previews" });
    expect(chip.querySelector(".mini-preview-folder")).toHaveAttribute("data-pose", "parked");
    expect(chip.querySelectorAll(".mini-preview-folder-sheet")).toHaveLength(2);
    fireEvent.click(chip);
    expect(chip).toHaveClass("mini-previews-hidden-restoring");
    expect(chip.querySelector(".mini-preview-folder")).toHaveAttribute("data-pose", "parked");
    expect(chip).toBeDisabled();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("restore_mini_previews");
  });

  it("keeps a singular URL count instead of the mock artifact list", async () => {
    window.history.replaceState({}, "", "/?view=mini-previews-hidden&count=1");
    render(<MiniPreviewsHiddenChip />);

    expect(screen.getByRole("button", { name: "Show 1 preview" })).toBeInTheDocument();
    await waitFor(() => expect(vi.mocked(listen)).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: "Show 1 preview" })).toBeInTheDocument();
    await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("get_artifacts"));
    expect(screen.getByRole("button", { name: "Show 1 preview" })).toBeInTheDocument();
  });

  it("updates the count when the parked stack changes", async () => {
    type CountHandler = (event: { payload: number }) => void;
    const countHandler: { current: CountHandler | null } = { current: null };
    vi.mocked(listen).mockImplementation(async (event, handler) => {
      if (event === "mini-previews-hidden-count") {
        countHandler.current = handler as CountHandler;
      }
      return () => undefined;
    });

    render(<MiniPreviewsHiddenChip />);
    expect(screen.getByRole("button", { name: "Show 2 previews" })).toBeInTheDocument();

    await waitFor(() => expect(countHandler.current).not.toBeNull());
    countHandler.current?.({ payload: 1 });

    expect(await screen.findByRole("button", { name: "Show 1 preview" })).toBeInTheDocument();
  });

  it("cleans up listeners that finish registering after unmount", async () => {
    type Unlisten = () => void;
    const pendingListeners: Array<(unlisten: Unlisten) => void> = [];
    const unlisten = vi.fn();
    vi.mocked(listen).mockImplementation(
      () => new Promise<Unlisten>((resolve) => pendingListeners.push(resolve)),
    );

    const { unmount } = render(<MiniPreviewsHiddenChip />);
    await waitFor(() => expect(pendingListeners).toHaveLength(3));
    unmount();

    await act(async () => {
      pendingListeners.forEach((resolve) => resolve(unlisten));
      await Promise.resolve();
    });

    expect(unlisten).toHaveBeenCalledTimes(3);
  });
});
