import { invoke, isTauri } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow, currentMonitor } from "@tauri-apps/api/window";
import { message, open } from "@tauri-apps/plugin-dialog";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import {
  type CSSProperties,
  type ReactNode,
  type RefObject,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { CompressionPreview } from "./CompressionPreview";
import { CustomSelect } from "./CustomSelect";
import { Feedback } from "./Feedback";
import { NumberInput } from "./NumberInput";
import { Onboarding } from "./Onboarding";
import { ScreenshotEditor } from "./ScreenshotEditor";
import { RangeSlider } from "./RangeSlider";
import {
  APPEARANCE_MODES,
  applyAppearance,
  DEFAULT_APPEARANCE,
  watchSystemAppearance,
  type AppearanceMode,
} from "../../../../shared/appearance";
import {
  applyColorTheme,
  buildCustomThemeVariables,
  COLOR_THEMES,
  DEFAULT_COLOR_THEME,
  DEFAULT_CUSTOM_THEME,
  normalizeCustomThemeColors,
  normalizeHexColor,
} from "../../../../shared/themes";
import { formatFileSize, formatFileSizeDelta, formatUpdateSize } from "./lib/format";
import { reconcileClipboardState } from "./lib/clipboard";
import { createCleanupRegistry } from "./lib/cleanupRegistry";
import { stackedReleaseNotes } from "./lib/releaseNotes";
import {
  editorCropAfterDrag,
  formatEditorTime,
  recordingEditedFileStem,
  recordingFilenameError,
  recordingUserFacingDefaults,
  timelineKeyboardDelta,
  type EditorCropHandle,
} from "./lib/recordingEditor";
import { isPointerOverCaptureGuidance } from "./lib/captureGuidance";
import {
  captureDimClipPath,
  constrainSelectionToAspect,
  dragSelectionRect,
  frontmostCaptureTargetAtPoint,
  frontToBackWindows,
  isCapturableSelection,
  keepReadyWindowTargets,
  parseAspectRatioPreset,
  REGION_ASPECT_PRESETS,
  roundedRectPath,
  windowListingIsReady,
  windowPointerHoverAtPoint,
  type RegionAspectPreset,
  type SelectionDragMode,
  type SelectionPoint,
  type SelectionRect,
} from "./lib/selection";
import {
  collectPreferenceFindTargets,
  matchPreferenceFindTargets,
  preferenceFindCountLabel,
  preferencesFindCommand,
  wrapFindIndex,
} from "./lib/preferencesFind";
import {
  detectShortcutPlatform,
  eventMatchesShortcut,
  isCaptureEscapeKey,
  isModifierCode,
  modifierDisplayTokens,
  platformShortcutHelp,
  recordShortcut,
  shortcutDisplayTokens,
} from "./lib/shortcut";
import {
  applyThumbnailCssCursor,
  applyThumbnailNativeHover,
  armThumbnailCollapsedHover,
  clearThumbnailCssCursor,
  clearThumbnailNativeHover,
  markThumbnailEditorControlOpened,
  rearmThumbnailEditorControlHover,
  releaseThumbnailCapturedHover,
  releaseThumbnailPointerCapture,
  retainThumbnailPointerCapture,
  thumbnailLostPointerCaptureShouldEndDrag,
  setThumbnailCardHoverSuppressed,
  setThumbnailCollapsedHoverStale,
  setThumbnailNativeActiveCard,
  shouldIgnoreThumbnailCursorEvents,
  shouldLockThumbnailCardHoverOnStackMotion,
  shouldRecoverThumbnailAfterNullPolls,
  thumbnailCardHoverLockReleased,
  thumbnailCursorSyncAction,
  thumbnailNullPollNeedsDesktopInputRecovery,
  thumbnailStackHasLiveHitTarget,
  thumbnailStackHoldsCollapsedPose,
  thumbnailUnknownPointerShouldIgnoreCursorEvents,
  withThumbnailPointerTimeout,
  THUMBNAIL_CURSOR_HANDOFF_REASSERT_DELAYS_MS,
  type ThumbnailCursorKind,
} from "./lib/thumbnailHover";
import {
  buildThumbnailDustParticles,
  playThumbnailDustAnimations,
  prefersReducedMotion,
  THUMBNAIL_CARD_FALLBACK_HEIGHT,
  THUMBNAIL_CARD_FALLBACK_WIDTH,
  THUMBNAIL_DELETE_ORIGIN_AFTER_CLOSE_X,
  THUMBNAIL_DELETE_ORIGIN_FIRST_X,
  THUMBNAIL_DELETE_ORIGIN_Y,
  type ThumbnailDustParticle,
} from "./lib/thumbnailExit";
import {
  isPreviewFileDropLanding,
  previewFileDropShouldDismiss,
  previewFileDropShouldReject,
  THUMBNAIL_DROP_REJECT_ANIMATION,
  THUMBNAIL_DROP_REJECT_MS,
} from "./lib/thumbnailFileDrag";
import {
  CollapsedThumbnailStackDrag,
  applyThumbnailStackDragSway,
  clampThumbnailStackFrame,
  cssUrl,
  preventThumbnailHtml5Drag,
  readHarnessStackOffset,
  setThumbnailStackDragSwayReady,
  setThumbnailStackDragging,
  setThumbnailStackPressing,
  thumbnailStackMeasuredFrameHeight,
  writeHarnessStackOffset,
} from "./lib/thumbnailStackDrag";
import {
  animateThumbnailStackScroll,
  applyThumbnailStackGravity,
  convertHarnessStackOffsetAnchor,
  convertThumbnailStackFrameAnchor,
  createThumbnailStackShiftController,
  DEFAULT_MINI_PREVIEW_PLACEMENT,
  harnessOffsetForPlacement,
  MINI_PREVIEW_PLACEMENTS,
  scheduleScrollThumbnailStackToNewest,
  scrollThumbnailStackToNewest,
  shouldScrollThumbnailStackToEnd,
  thumbnailCollapsedFrameHeight,
  thumbnailStackAnchorFromGravity,
  thumbnailStackAnchorFromPlacement,
  thumbnailStackBiasFromFrameX,
  thumbnailStackBiasFromHarness,
  thumbnailStackContentHeight,
  thumbnailStackGravityFromHarness,
  thumbnailStackGravityFromPlacement,
  thumbnailStackGravityFromWorkArea,
  thumbnailStackSideFromBias,
  thumbnailStackSideFromPlacement,
  thumbnailStackVisualPileBottom,
  thumbnailStackNeedsScrollport,
  thumbnailStackOverflow,
  restoreThumbnailStackShiftClass,
  thumbnailCollapsedPeekPx,
  captureThumbnailCardTransforms,
  thumbnailStackFanCollapseMs,
  thumbnailStackPeekJitterPx,
  THUMBNAIL_CARD_SLOT_PX,
  THUMBNAIL_STACK_EXPAND_COLLAPSE_MS,
  THUMBNAIL_STACK_SCROLLPORT_CLASS,
  waitForThumbnailStackSettle,
  type ThumbnailStackAnchor,
  type ThumbnailStackSide,
} from "./lib/thumbnailLayout";
import {
  EDITOR_PRESENCE_LEAVE_MS,
  EDITOR_PRESENCE_LINGER_MS,
  artifactIdsInEditors,
  reconcileEditorPresence,
} from "./lib/editorPresence";
import { reconcileActiveViewer } from "./lib/viewerActivation";
import type {
  ActiveSession,
  AudioDevice,
  AppSettings,
  ArtifactDragPayload,
  PreviewFileDropLanding,
  ArtifactSummary,
  CaptureArtifact,
  CaptureMode,
  ClipboardState,
  CustomThemeColors,
  EditSpec,
  ExportProgress,
  ExportSpec,
  MaxResolution,
  MiniPreviewPlacement,
  RecordingArtifact,
  RecordingDraftManifest,
  RecordingOptions,
  RecordingSelectionSession,
  RecordingSessionSnapshot,
  RecordingTarget,
  RecordingTimelinePreview,
  ScreenshotFormat,
  ThumbnailPointerPosition,
  UpdateStatus,
  EditorLayerPresence,
  VideoFormat,
  ViewerActivationState,
} from "./types";

const currentWindow = isTauri() ? getCurrentWindow() : null;

function dismissCaptureOverlayWindow() {
  // Drop native keyboard grabs in parallel with Tauri hide. Waiting for the
  // async capture command left a hidden key window that swallowed typing in
  // other apps after a few screenshots.
  void invoke("dismiss_capture_surface").catch(() => undefined);
  void currentWindow?.hide().catch(() => undefined);
}

function onCaptureEscape(handler: () => void): () => void {
  const onKeyDown = (event: KeyboardEvent) => {
    if (!isCaptureEscapeKey(event)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    handler();
  };
  window.addEventListener("keydown", onKeyDown, true);
  return () => window.removeEventListener("keydown", onKeyDown, true);
}

function overlayPointFromClient(
  surface: HTMLElement | null,
  clientX: number,
  clientY: number,
): SelectionPoint {
  const bounds = surface?.getBoundingClientRect();
  return {
    x: clientX - (bounds?.left ?? 0),
    y: clientY - (bounds?.top ?? 0),
  };
}

function capturableOverlayWindows<T extends { width: number; height: number }>(
  windows: readonly T[],
): T[] {
  return windows.filter((item) => item.width >= 48 && item.height >= 48);
}

function requestCapturePointerPosition(): Promise<ThumbnailPointerPosition | null> {
  return invoke<ThumbnailPointerPosition | null>("get_capture_pointer_position")
    .catch(() => null);
}
// Slightly past dismiss hold (450ms fade + 580ms shared settle) so animationend
// remains the primary completion path; fallback only covers missed events.
const THUMBNAIL_DISMISS_FALLBACK_MS = 1_250;
const THUMBNAIL_DELETE_FALLBACK_MS = 3_200;
/** Newest card leaves first; cap so a tall stack still finishes with the fallback. */
const THUMBNAIL_CLEAR_STAGGER_MS = 36;
const THUMBNAIL_CLEAR_STAGGER_MAX_MS = 180;
/** Sentinel so a stack Clear all does not issue per-card dismiss_artifact. */
const STACK_CLEAR_EXIT_ACTION = "stack_clear";
/** How long the mini-preview shows “Saved” before flipping to “Show in Folder”. */
const THUMBNAIL_SAVED_FEEDBACK_MS = 1_000;
const THUMBNAIL_HIT_TEST_CHANGED_EVENT = "captures-thumbnail-hit-test-changed";
const RECORDING_SELECTOR_REVEAL_FALLBACK_MS = 200;
/** Hidden overlay paints first; reveal after decode or this deadline. Tests use a longer deadline so jsdom cannot race the wake assertion. */
const CAPTURE_OVERLAY_REVEAL_FALLBACK_MS = import.meta.env.MODE === "test" ? 10_000 : 400;
const RECORDING_COUNTDOWN_FADE_OUT_MS = 180;
const PREFERENCES_TARGET_EVENT = "preferences-target";
const AUTO_START_PREFERENCE_TARGET = "auto-start-on-selection";
const AUTO_START_PREFERENCE_ID = "auto-start-on-selection-setting";
const RECORDING_CONTROLS_PREFERENCE_TARGET = "include-recording-controls-in-captures";
const RECORDING_CONTROLS_PREFERENCE_ID = "include-recording-controls-in-captures-setting";
const PREFERENCE_TARGET_IDS: Record<string, string> = {
  [AUTO_START_PREFERENCE_TARGET]: AUTO_START_PREFERENCE_ID,
  [RECORDING_CONTROLS_PREFERENCE_TARGET]: RECORDING_CONTROLS_PREFERENCE_ID,
};
const PREFERENCE_HIGHLIGHT_MS = 2_400;
const COUNTDOWN_SECONDS = Array.from({ length: 11 }, (_, seconds) => seconds);

function query(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name);
}

function afterNextPaint(callback: () => void) {
  requestAnimationFrame(() => requestAnimationFrame(callback));
}

/** Frozen overlays wait for the snapshot image; live selectors use a synthetic key. */
function freezeFrameRevealKey(session: { frozen?: boolean; snapshot_url: string }): string {
  return session.frozen === false ? (session.snapshot_url || "live") : session.snapshot_url;
}

function sessionShowsFreezeFrame(session: { frozen?: boolean } | null | undefined): boolean {
  return session?.frozen !== false;
}

function emitViewerActivation(artifactId: string | null, active: boolean) {
  if (!artifactId) return;
  void emit<ViewerActivationState>("viewer-activation-changed", {
    artifact_id: artifactId,
    active,
  }).catch(() => undefined);
}

/**
 * Keeps every window on the same appearance and accent as the saved settings.
 * Runs in the browser too so the shared "System" preference still tracks the OS.
 */
function useAppearanceSync() {
  useEffect(() => {
    let active = true;
    let mode: AppearanceMode = DEFAULT_APPEARANCE;
    let unlisten: (() => void) | undefined;

    const stopWatching = watchSystemAppearance(() => {
      if (active && mode === "system") applyAppearance("system");
    });

    if (!isTauri()) {
      return () => {
        active = false;
        stopWatching();
      };
    }

    const applySettings = (settings: AppSettings) => {
      mode = settings.appearance ?? DEFAULT_APPEARANCE;
      applyAppearance(mode);
      applyColorTheme(
        settings.theme ?? DEFAULT_COLOR_THEME,
        settings.custom_theme ?? DEFAULT_CUSTOM_THEME,
      );
    };

    void invoke<AppSettings>("get_settings")
      .then((settings) => {
        if (active) applySettings(settings);
      })
      .catch(() => undefined);

    void listen<AppSettings>("settings-changed", ({ payload }) => {
      if (active) applySettings(payload);
    }).then((dispose) => {
      if (active) unlisten = dispose;
      else dispose();
    }).catch(() => undefined);

    return () => {
      active = false;
      stopWatching();
      unlisten?.();
    };
  }, []);
}

export function App() {
  useAppearanceSync();
  const view = query("view");
  if (view === "overlay") return <CaptureOverlay />;
  if (view === "recording-selector") return <RecordingSelector />;
  if (view === "recording-region-indicator") return <RecordingRegionIndicator />;
  if (view === "recording-countdown") return <RecordingCountdown />;
  if (view === "screenshot-countdown") return <ScreenshotCountdown />;
  if (view === "recording-hud") return <RecordingHud />;
  if (view === "recording-editor") return <RecordingEditor />;
  if (view === "screenshot-editor") return <ScreenshotEditor />;
  if (view === "recording-saved") return <RecordingSavedNotice />;
  if (view === "recording-controls-hidden") return <RecordingControlsHiddenNotice />;
  if (view === "thumbnail") return <Thumbnail />;
  if (view === "viewer") return <ArtifactViewer />;
  if (view === "history") return <CaptureHistory />;
  if (view === "preferences") return <Preferences />;
  if (view === "feedback") return <Feedback />;
  if (view === "onboarding") return <Onboarding />;
  if (view === "startup") return <StartupNotice />;
  if (view === "update") return <UpdateNotice />;
  return <IdleView />;
}

function captureChromeLabel(): string {
  return /Mac/i.test(navigator.userAgent) ? "menu bar" : "tray";
}

function IdleView() {
  return (
    <main className="idle-view">
      <div className="brand-mark">Captures</div>
      <h1>Captures is running</h1>
      <p>Use the Captures menu or your New Capture shortcut to start a capture.</p>
    </main>
  );
}

type TrayNoticeCaret = { edge: "top" | "bottom"; x: number };

function parseTrayNoticeCaret(): TrayNoticeCaret | null {
  const caretEdge = query("caret");
  const caretXRaw = query("caret_x");
  const caretX = caretXRaw == null ? Number.NaN : Number(caretXRaw);
  if ((caretEdge !== "top" && caretEdge !== "bottom") || !Number.isFinite(caretX)) {
    return null;
  }
  return { edge: caretEdge, x: caretX };
}

function useTrayNoticeCaret() {
  const [caret, setCaret] = useState<TrayNoticeCaret | null>(parseTrayNoticeCaret);

  useEffect(() => {
    let active = true;
    const cleanup = createCleanupRegistry();
    void listen<TrayNoticeCaret>("notice-caret", ({ payload }) => {
      if (active && (payload.edge === "top" || payload.edge === "bottom")) {
        setCaret(payload);
      }
    }).then((unlisten) => {
      cleanup.add(unlisten);
    }).catch(() => undefined);
    return () => {
      active = false;
      cleanup.dispose();
    };
  }, []);

  return caret;
}

function TrayNoticeShell({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  const caret = useTrayNoticeCaret();
  const caretStyle = caret
    ? ({ "--tray-caret-x": `${caret.x}px` } as CSSProperties)
    : undefined;

  return (
    <div
      className={["tray-notice", className].filter(Boolean).join(" ")}
      data-caret={caret?.edge}
      style={caretStyle}
    >
      {caret ? <div className="tray-notice-caret" aria-hidden="true" /> : null}
      {children}
    </div>
  );
}

export function StartupNotice() {
  return (
    <TrayNoticeShell className="startup-notice">
      <p className="startup-notice-card" role="status">
        Captures is ready to use
      </p>
    </TrayNoticeShell>
  );
}

export function RecordingSavedNotice() {
  const [notice, setNotice] = useState({
    artifactId: query("artifact_id"),
    generation: 0,
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [permanentlySaved, setPermanentlySaved] = useState(false);
  const artifactId = notice.artifactId;

  useEffect(() => {
    let active = true;
    const cleanup = createCleanupRegistry();
    void listen<{ artifact_id: string; generation: number }>(
      "recording-saved-artifact",
      ({ payload }) => {
        if (!active) return;
        setNotice({ artifactId: payload.artifact_id, generation: payload.generation });
        setBusy(false);
        setError("");
        setPermanentlySaved(false);
      },
    ).then((unlisten) => {
      cleanup.add(unlisten);
    }).catch(() => undefined);
    return () => {
      active = false;
      cleanup.dispose();
    };
  }, []);

  useEffect(() => {
    if (!artifactId) return;
    let active = true;
    void invoke<{ saved_path?: string | null } | null>("get_recording_artifact", { artifactId })
      .then((artifact) => {
        if (active) setPermanentlySaved(Boolean(artifact?.saved_path));
      })
      .catch(() => {
        if (active) setPermanentlySaved(false);
      });
    return () => {
      active = false;
    };
  }, [artifactId, notice.generation]);

  const dismiss = () => {
    void invoke("dismiss_recording_saved_notice");
  };

  const reveal = async () => {
    if (!artifactId || busy) return;
    setBusy(true);
    setError("");
    try {
      await invoke("reveal_recording_artifact", { artifactId });
      dismiss();
    } catch (error) {
      setError(`Could not show the recording in its folder: ${String(error)}`);
      setBusy(false);
    }
  };

  const save = async () => {
    if (!artifactId || busy) return;
    setBusy(true);
    setError("");
    try {
      await invoke("save_recording_artifact", { artifactId });
      setPermanentlySaved(true);
      setBusy(false);
    } catch (error) {
      setError(`Could not save the recording: ${String(error)}`);
      setBusy(false);
    }
  };

  return (
    <main key={notice.generation} className="recording-saved-notice">
      <span className="recording-saved-icon" aria-hidden="true"><CheckIcon /></span>
      <div className="recording-saved-copy">
        <strong>{permanentlySaved ? "Recording saved" : "Recording ready"}</strong>
        <p>
          {error
            || (permanentlySaved
              ? "Saved to your Captures folder."
              : "Kept in Capture History for 30 days. Save a copy anytime.")}
        </p>
      </div>
      {permanentlySaved ? (
        <button
          type="button"
          className="recording-saved-reveal"
          disabled={busy || !artifactId}
          onClick={() => void reveal()}
        ><FolderIcon />{busy ? "Opening…" : "Show in Folder"}</button>
      ) : (
        <button
          type="button"
          className="recording-saved-reveal"
          disabled={busy || !artifactId}
          onClick={() => void save()}
        ><SaveIcon />{busy ? "Saving…" : "Save file"}</button>
      )}
      <button
        type="button"
        className="recording-saved-dismiss"
        aria-label="Dismiss"
        onClick={dismiss}
      >×</button>
    </main>
  );
}

export function RecordingControlsHiddenNotice() {
  const [shortcut, setShortcut] = useState("CommandOrControl+Shift+Space");

  useEffect(() => {
    void invoke<AppSettings>("get_settings")
      .then((settings) => {
        if (settings.new_capture_shortcut.trim()) {
          setShortcut(settings.new_capture_shortcut);
        }
      })
      .catch(() => undefined);
  }, []);

  const keys = shortcutDisplayTokens(shortcut);
  const trayLabel = captureChromeLabel();

  return (
    <main className="recording-controls-hidden-notice" role="status">
      <span className="recording-controls-hidden-icon" aria-hidden="true"><CaptureIcon /></span>
      <div>
        <strong>Recording controls hidden</strong>
        <p>
          Open Captures from the {trayLabel}, or press{" "}
          {keys.map((key, index) => (
            <kbd key={`${key}-${index}`}>{key}</kbd>
          ))}
          {" "}to bring them back.
        </p>
      </div>
    </main>
  );
}

function useUpdateStatus() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    void invoke<UpdateStatus>("get_update_status")
      .then((loaded) => {
        if (active) setStatus(loaded);
      })
      .catch(() => undefined);
    void listen<UpdateStatus>("update-status-changed", ({ payload }) => {
      if (active) setStatus(payload);
    }).then((dispose) => {
      if (active) unlisten = dispose;
      else dispose();
    }).catch(() => undefined);
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  return status;
}

const OPEN_CAPTURES_UPDATE_WARNING =
  "Open captures will close. Unsaved edits are kept as drafts.";

function UpdateDownloadFallback({ source }: { source: "notice" | "preferences" }) {
  const download = (
    <button
      type="button"
      className="update-download-fallback-link"
      onClick={() => void invoke("open_update_download_page")}
    >
      download from captur.es
    </button>
  );
  const linuxRecovery = detectShortcutPlatform() === "linux";
  return (
    <p className="update-download-fallback">
      {source === "preferences" ? (
        linuxRecovery ? (
          <>
            If this copy cannot update itself, {download}. Debian packages replace this
            app; AppImage users should replace ~/.local/bin/Captures.AppImage. Settings
            and captures stay.
          </>
        ) : (
          <>
            If this copy cannot update itself, {download} and install over it. Settings and
            captures stay.
          </>
        )
      ) : (
        <>You can also {download}.</>
      )}
    </p>
  );
}

export function UpdateNotice() {
  const status = useUpdateStatus();
  const [showChangelog, setShowChangelog] = useState(true);
  const [actionError, setActionError] = useState("");
  const [installing, setInstalling] = useState(false);
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let active = true;
    const cleanup = createCleanupRegistry();
    void invoke<AppSettings>("get_settings")
      .then((settings) => {
        if (active) setShowChangelog(settings.show_update_changelog !== false);
      })
      .catch(() => undefined);
    void listen<AppSettings>("settings-changed", ({ payload }) => {
      if (active) setShowChangelog(payload.show_update_changelog !== false);
    }).then((unlisten) => {
      cleanup.add(unlisten);
    }).catch(() => undefined);
    return () => {
      active = false;
      cleanup.dispose();
    };
  }, []);

  const persistShowChangelog = (next: boolean) => {
    setShowChangelog(next);
    void invoke<AppSettings>("get_settings")
      .then((current) => invoke("update_settings", {
        settings: { ...current, show_update_changelog: next },
      }))
      .catch(() => undefined);
  };

  const close = () => {
    setActionError("");
    void invoke("dismiss_update_notice");
  };
  const run = async (command: "check_for_updates" | "install_update") => {
    setActionError("");
    if (command === "install_update") setInstalling(true);
    try {
      await invoke(command);
    } catch (error) {
      setActionError(String(error));
    } finally {
      if (command === "install_update") setInstalling(false);
    }
  };

  const available = status?.state === "available" ? status : null;
  const installableDownloadSize = available?.installable ? available.download_size : null;
  const downloading = status?.state === "downloading" ? status : null;
  const restarting = status?.state === "restarting" ? status : null;
  const error = actionError || (status?.state === "error" ? status.message : "");
  const groups = available
    ? stackedReleaseNotes(available.changelog, available.notes, available.display_version)
    : [];
  const stacked = groups.length > 1;
  const notesVisible = Boolean(available && !error && showChangelog);
  const progress = downloading?.total
    ? Math.min(100, Math.round((downloading.downloaded / downloading.total) * 100))
    : null;
  const downloadProgress = downloading
    ? downloading.total
      ? `${formatUpdateSize(downloading.downloaded)} / ${formatUpdateSize(downloading.total)}`
      : `${formatUpdateSize(downloading.downloaded)} downloaded`
    : "";
  const state = status?.state ?? "loading";
  const visualState = error ? "error" : state;
  const title = restarting
    ? "Updated"
    : downloading
      ? "Updating Captures"
      : error
        ? "Update failed"
        : available
          ? "Update available"
          : status?.state === "up_to_date"
            ? "You’re up to date"
            : status?.state === "checking"
              ? "Checking for updates"
              : "Loading update details";
  const description = restarting
    ? `Version ${restarting.display_version}`
    : downloading
      ? `Version ${downloading.display_version}`
      : error
        ? "Your current version was not changed."
        : available
          ? `Version ${available.display_version}${installableDownloadSize ? ` · ${formatUpdateSize(installableDownloadSize)}` : ""}`
          : status?.state === "up_to_date"
            ? `Version ${status.current_display_version}`
            : "This should only take a moment.";
  const dismissBlocked = Boolean(downloading || restarting || installing);

  useLayoutEffect(() => {
    const root = dialogRef.current;
    if (!root) return;
    const primary = root.querySelector<HTMLButtonElement>("button.primary");
    const dismiss = root.querySelector<HTMLButtonElement>("button.update-dismiss");
    (primary ?? dismiss ?? root).focus({ preventScroll: true });
  }, [visualState, available, error, downloading, restarting, notesVisible]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (dismissBlocked) return;
      event.preventDefault();
      close();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [dismissBlocked]);

  return (
    <TrayNoticeShell>
      <div
        ref={dialogRef}
        className={[
          "update-notice",
          `update-notice-${visualState}`,
          "tray-notice-card",
          notesVisible ? "" : "update-notice-compact",
        ].filter(Boolean).join(" ")}
        role="dialog"
        tabIndex={-1}
        aria-labelledby="update-notice-title"
        aria-describedby="update-notice-description"
      >
      <header className="update-notice-header">
        <div className="update-app-icon" aria-hidden="true">
          {visualState === "restarting" ? (
            <CheckIcon />
          ) : visualState === "error" ? (
            <WarningIcon />
          ) : visualState === "downloading" || visualState === "checking" || visualState === "loading" ? (
            <span className="update-spinner" />
          ) : (
            <CaptureIcon />
          )}
        </div>
        <div className="update-notice-copy">
          <h1 id="update-notice-title">{title}</h1>
          <p id="update-notice-description">{description}</p>
        </div>
        {available && !error && !notesVisible && (
          <button
            type="button"
            className="update-notes-reveal"
            onClick={() => persistShowChangelog(true)}
          >
            What’s new
          </button>
        )}
      </header>

      <div className={`update-notice-body${notesVisible ? "" : " update-notice-body-status"}`}>
        {notesVisible && (
          <section className={`update-notes${stacked ? " update-notes-stacked" : ""}`} aria-label="What's new">
            <div className="update-notes-heading">
              <h2>What’s new</h2>
              <button
                type="button"
                className="update-notes-toggle"
                onClick={() => persistShowChangelog(false)}
              >
                Hide
              </button>
            </div>
            {stacked && (
              <p className="update-notes-intro">
                This update includes all of the following changes:
              </p>
            )}
            {groups.length > 0 ? (
              <div className="update-notes-scroll">
                {groups.map((group) => {
                  const headingId = `update-notes-${group.version || group.displayVersion}`;
                  return (
                    <section
                      key={group.version || group.displayVersion}
                      className="update-notes-group"
                      aria-labelledby={stacked ? headingId : undefined}
                    >
                      {stacked && <h3 id={headingId}>{group.displayVersion}</h3>}
                      {group.items.length > 0 ? (
                        <ul className="update-notes-list">
                          {group.items.map((note, index) => {
                            const pullRequest = note.pullRequest;
                            return (
                              <li key={`${group.version}-${index}-${note.text}`}>
                                {note.text}
                                {pullRequest ? (
                                  <>
                                    {" "}
                                    <button
                                      type="button"
                                      className="update-notes-pr"
                                      aria-label={`Open pull request ${pullRequest.number}`}
                                      onClick={() => void invoke("open_update_changelog_url", {
                                        url: pullRequest.url,
                                      })}
                                    >
                                      #{pullRequest.number}
                                    </button>
                                  </>
                                ) : null}
                              </li>
                            );
                          })}
                        </ul>
                      ) : (
                        <p className="update-notes-empty">
                          Release notes aren’t available for this Preview.
                        </p>
                      )}
                    </section>
                  );
                })}
              </div>
            ) : (
              <p className="update-notes-empty">Release notes aren’t available for this update.</p>
            )}
          </section>
        )}

        {downloading && (
          <section className="update-download" aria-label="Update installation progress">
            <div className="update-progress-label">
              <span>Downloading <small>{downloadProgress}</small></span>
              {progress !== null && <strong>{progress}%</strong>}
            </div>
            <div
              className={`update-progress${progress === null ? " update-progress-indeterminate" : ""}`}
              role="progressbar"
              aria-label="Downloading update"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={progress ?? undefined}
              aria-valuetext={progress === null ? downloadProgress : `${downloadProgress}, ${progress}% downloaded`}
            >
              <span style={{ width: `${progress ?? 34}%` }} />
            </div>
          </section>
        )}

        {restarting && (
          <section className="update-restart" role="status">
            <p className="update-status-message update-restarting">
              Reopening in {restarting.seconds_remaining} seconds…
            </p>
            <div
              className="update-restart-progress"
              aria-hidden="true"
            >
              <span />
            </div>
          </section>
        )}

        {error && (
          <>
            <p className="update-error" role="alert">{error}</p>
            <UpdateDownloadFallback source="notice" />
          </>
        )}

        {!available && !downloading && !restarting && !error && status?.state === "up_to_date" && (
          <p className="update-status-message" role="status">No updates are available.</p>
        )}

        {!available && !downloading && !restarting && !error && status?.state !== "up_to_date" && (
          <p className="update-status-message update-checking" role="status">
            Checking…
          </p>
        )}
      </div>

      {available && !error && available.will_close_open_captures && (
        <p className="update-close-warning" role="status">
          <span className="update-close-warning-icon" aria-hidden="true"><WarningIcon /></span>
          {OPEN_CAPTURES_UPDATE_WARNING}
        </p>
      )}

      {!downloading && !restarting && (
        <footer className="update-notice-footer">
          <button
            className="update-dismiss"
            type="button"
            disabled={installing}
            onClick={close}
          >
            {available ? "Later" : "Close"}
          </button>
          {available && !error && (
            <button
              className="primary"
              type="button"
              disabled={installing}
              onClick={() => void run("install_update")}
            >
              {available.installable ? "Update now" : "View release"}
            </button>
          )}
          {error && (
            <button
              className="primary"
              type="button"
              disabled={installing}
              onClick={() =>
                void run(
                  available || (status?.state === "error" && status.retry_install)
                    ? "install_update"
                    : "check_for_updates",
                )
              }
            >
              Try again
            </button>
          )}
          {!available && !error && status?.state !== "checking" && status?.state !== "up_to_date" && (
            <button className="primary" type="button" onClick={() => void run("check_for_updates")}>
              Check again
            </button>
          )}
        </footer>
      )}
      </div>
    </TrayNoticeShell>
  );
}

function UpdatePreferences({
  showChangelog,
  updateShowChangelog,
}: {
  showChangelog: boolean;
  updateShowChangelog: (value: boolean) => void;
}) {
  const status = useUpdateStatus();
  const [actionError, setActionError] = useState("");
  const currentVersion = status?.current_display_version ?? "…";
  const available = status?.state === "available" ? status : null;
  const installableDownloadSize = available?.installable ? available.download_size : null;
  const downloading = status?.state === "downloading" ? status : null;
  const restarting = status?.state === "restarting" ? status : null;
  const progress = downloading?.total
    ? Math.min(100, Math.round((downloading.downloaded / downloading.total) * 100))
    : null;
  const downloadProgress = downloading
    ? downloading.total
      ? `${formatUpdateSize(downloading.downloaded)} / ${formatUpdateSize(downloading.total)}`
      : `${formatUpdateSize(downloading.downloaded)} downloaded`
    : "";
  const heading = available
    ? `Version ${available.display_version} is available`
    : downloading
      ? `Updating to version ${downloading.display_version}`
      : restarting
        ? "Update complete"
        : status?.state === "checking"
          ? "Checking for updates…"
          : status?.state === "error"
            ? "Couldn’t check for updates"
            : `Version ${currentVersion}`;
  const detail = downloading
    ? progress === null ? downloadProgress : `${downloadProgress} · ${progress}%`
    : restarting
      ? "Reopening Captures…"
      : status?.state === "up_to_date"
        ? "Up to date."
        : status?.state === "error"
          ? status.message
          : available
            ? installableDownloadSize ? `${formatUpdateSize(installableDownloadSize)} download` : ""
            : status?.state === "checking"
              ? ""
            : "Updates are checked automatically.";

  const run = async (command: "check_for_updates" | "install_update") => {
    setActionError("");
    try {
      await invoke(command);
    } catch (error) {
      setActionError(String(error));
    }
  };

  return (
    <section className="settings-card update-settings" id="updates" aria-labelledby="updates-heading">
      <header className="settings-card-header">
        <h2 id="updates-heading">Updates</h2>
        <p>
          Preview builds check for a new version automatically. Update now installs it in
          place.
        </p>
      </header>
      <div className="settings-utility-row update-settings-row">
        <div className="settings-utility-copy">
          <strong>{heading}</strong>
          {detail && <small>{detail}</small>}
        </div>
        <button
          className="settings-utility-action"
          type="button"
          disabled={Boolean(status?.state === "checking" || downloading || restarting)}
          onClick={() =>
            void run(
              available || (status?.state === "error" && status.retry_install)
                ? "install_update"
                : "check_for_updates",
            )
          }
        >
          {restarting
            ? "Restarting…"
            : downloading
            ? "Installing…"
            : available
              ? available.installable ? "Update now" : "View release"
              : status?.state === "checking" ? "Checking…" : "Check Now"}
        </button>
        {downloading && (
          <div
            className={`update-settings-progress${progress === null ? " update-settings-progress-indeterminate" : ""}`}
            role="progressbar"
            aria-label="Installing update"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={progress ?? undefined}
          >
            <span style={{ width: `${progress ?? 34}%` }} />
          </div>
        )}
      </div>
      {available?.will_close_open_captures && !actionError && (
        <p className="update-settings-warning">{OPEN_CAPTURES_UPDATE_WARNING}</p>
      )}
      {actionError && <p className="update-settings-error" role="alert">{actionError}</p>}
      <UpdateDownloadFallback source="preferences" />
      <label className="check-row switch-row">
        <input
          type="checkbox"
          checked={showChangelog}
          onChange={(event) => updateShowChangelog(event.target.checked)}
        />
        <span>
          Show what’s new on update notices
          <small>
            Lists every Preview since the version you have. Turn this off for a
            compact Update now prompt.
          </small>
        </span>
      </label>
    </section>
  );
}

function ArtifactViewer() {
  const artifactId = query("artifact_id");
  const [artifact, setArtifact] = useState<CaptureArtifact | null>(null);
  const [fit, setFit] = useState(true);
  const [openingEditor, setOpeningEditor] = useState(false);
  const [editorError, setEditorError] = useState("");

  useEffect(() => {
    let active = true;
    const cleanup = createCleanupRegistry();
    void (async () => {
      const listeners = await Promise.all([
        listen<string>("artifact-removed", ({ payload }) => {
          if (!active) return;
          if (artifactId !== payload) return;
          emitViewerActivation(artifactId, false);
          void currentWindow?.close();
        }),
      ]);
      if (!cleanup.add(...listeners)) return;
      if (!artifactId) return;
      const initialArtifact = await invoke<CaptureArtifact | null>("get_artifact", { artifactId });
      if (active) {
        setArtifact(initialArtifact);
      }
    })();
    return () => {
      active = false;
      cleanup.dispose();
    };
  }, [artifactId]);

  useEffect(() => {
    if (!currentWindow) return;
    let active = true;
    const cleanup = createCleanupRegistry();

    void (async () => {
      const stopListening = await Promise.all([
        currentWindow.onFocusChanged(({ payload }) => {
          if (active && payload) emitViewerActivation(artifactId, true);
        }),
        currentWindow.onCloseRequested(() => {
          if (active) emitViewerActivation(artifactId, false);
        }),
      ]);
      if (!cleanup.add(...stopListening)) return;
      const focused = await currentWindow.isFocused();
      if (active && focused) emitViewerActivation(artifactId, true);
    })();

    return () => {
      active = false;
      cleanup.dispose();
    };
  }, [artifactId]);

  if (!artifact) return <main className="viewer-loading">Loading preview…</main>;

  return (
    <main className="artifact-viewer">
      <header className="viewer-toolbar">
        <div>
          <strong>Captures Preview</strong>
          <span>{artifact.width} × {artifact.height}</span>
        </div>
        <div className="viewer-toolbar-actions">
          <button
            type="button"
            disabled={openingEditor}
            onClick={() => {
              setOpeningEditor(true);
              setEditorError("");
              void invoke("open_screenshot_editor", { artifactId: artifact.id })
                .catch((error) => setEditorError(String(error)))
                .finally(() => setOpeningEditor(false));
            }}
          >
            <EditIcon />{openingEditor ? "Opening…" : "Edit"}
          </button>
          <button type="button" onClick={() => setFit((current) => !current)}>
            {fit ? "Actual size" : "Fit to window"}
          </button>
        </div>
      </header>
      {editorError && <p className="viewer-error" role="alert">{editorError}</p>}
      <div className="viewer-canvas" onDoubleClick={() => setFit((current) => !current)}>
        <img
          key={artifact.id}
          className={fit ? "viewer-image viewer-image-fit" : "viewer-image viewer-image-actual"}
          src={artifact.full_url}
          alt="Full-size screenshot"
          draggable={false}
        />
      </div>
    </main>
  );
}

const HISTORY_FILTERS = [
  { id: "all", label: "All" },
  { id: "screenshot", label: "Screenshots" },
  { id: "video", label: "Video" },
  { id: "gif", label: "GIF" },
] as const;

type HistoryFilter = (typeof HISTORY_FILTERS)[number]["id"];

export function CaptureHistory() {
  const [entries, setEntries] = useState<ArtifactSummary[]>([]);
  const [filter, setFilter] = useState<HistoryFilter>("all");
  const [drafts, setDrafts] = useState<RecordingDraftManifest[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [clearingAll, setClearingAll] = useState(false);
  const [confirmingClearAll, setConfirmingClearAll] = useState(false);
  const activeRef = useRef(true);
  const clearAllTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [history, interrupted] = await Promise.all([
        invoke<ArtifactSummary[]>("get_capture_history"),
        invoke<RecordingDraftManifest[]>("get_recording_drafts"),
      ]);
      if (!activeRef.current) return;
      setEntries(history);
      setDrafts(interrupted);
      setError("");
    } catch (error) {
      if (activeRef.current) setError(`Couldn’t load capture history: ${String(error)}`);
    } finally {
      if (activeRef.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    const cleanup = createCleanupRegistry();
    activeRef.current = true;

    void (async () => {
      const unlisten = await listen("capture-history-changed", () => {
        if (activeRef.current) void refresh();
      });
      if (!cleanup.add(unlisten)) return;
      await refresh();
    })();

    return () => {
      activeRef.current = false;
      cleanup.dispose();
      if (clearAllTimer.current) clearTimeout(clearAllTimer.current);
    };
  }, [refresh]);

  const clearAllHistory = async () => {
    if (clearingAll || entries.length === 0) return;
    if (!confirmingClearAll) {
      setConfirmingClearAll(true);
      if (clearAllTimer.current) clearTimeout(clearAllTimer.current);
      clearAllTimer.current = setTimeout(() => setConfirmingClearAll(false), 4_000);
      return;
    }

    setClearingAll(true);
    setError("");
    try {
      await invoke("clear_capture_history");
      if (!activeRef.current) return;
      setEntries([]);
      setConfirmingClearAll(false);
    } catch (error) {
      if (activeRef.current) {
        setError(`Couldn’t delete capture history: ${String(error)}`);
        setConfirmingClearAll(false);
      }
    } finally {
      if (activeRef.current) setClearingAll(false);
    }
  };

  const counts = {
    all: entries.length,
    screenshot: entries.filter((entry) => entry.kind === "screenshot").length,
    video: entries.filter((entry) => entry.kind === "video").length,
    gif: entries.filter((entry) => entry.kind === "gif").length,
  };
  const filtered = filter === "all"
    ? entries
    : entries.filter((entry) => entry.kind === filter);

  return (
    <main className="capture-history">
      <div className="history-shell">
        <header className="history-header">
          <div className="history-heading">
            <p className="eyebrow">On this device</p>
            <h1>Capture History</h1>
            <p>Screenshots, videos, GIFs, and interrupted recordings you can recover all appear here for 30 days.</p>
          </div>
          {!loading && entries.length > 0 && (
            <div className="history-header-actions">
              {confirmingClearAll && (
                <button
                  type="button"
                  className="history-clear-all-cancel"
                  aria-label="Cancel delete all captures"
                  disabled={clearingAll}
                  onClick={() => {
                    setConfirmingClearAll(false);
                    if (clearAllTimer.current) clearTimeout(clearAllTimer.current);
                  }}
                >
                  Cancel
                </button>
              )}
              <button
                type="button"
                className={confirmingClearAll
                  ? "history-clear-all history-clear-all-confirm"
                  : "history-clear-all"}
                aria-label={confirmingClearAll
                  ? "Confirm delete all captures"
                  : "Delete all captures"}
                disabled={clearingAll}
                onClick={() => void clearAllHistory()}
              >
                <TrashIcon />
                {clearingAll
                  ? "Deleting…"
                  : confirmingClearAll
                    ? "Delete all forever"
                    : "Delete all"}
              </button>
            </div>
          )}
        </header>

        {!loading && entries.length > 0 && (
          <div className="history-toolbar">
            <div className="history-filters" role="group" aria-label="Filter captures">
              {HISTORY_FILTERS.map((option) => (
                <button
                  key={option.id}
                  type="button"
                  className={filter === option.id ? "active" : ""}
                  aria-pressed={filter === option.id}
                  disabled={counts[option.id] === 0 && option.id !== "all"}
                  onClick={() => setFilter(option.id)}
                >
                  {option.label}
                  <span aria-hidden="true">{counts[option.id]}</span>
                </button>
              ))}
            </div>
          </div>
        )}

        {error && <p className="history-error" role="alert">{error}</p>}
        <RecordingRecovery drafts={drafts} onChanged={refresh} />
        {loading ? (
          <section className="history-empty" aria-live="polite">
            <span className="history-empty-icon" aria-hidden="true"><HistoryIcon /></span>
            <h2>Loading history…</h2>
          </section>
        ) : entries.length === 0 && drafts.length === 0 ? (
          <section className="history-empty">
            <span className="history-empty-icon" aria-hidden="true"><HistoryIcon /></span>
            <h2>No captures yet</h2>
            <p>New screenshots, videos, and GIFs appear here automatically.</p>
          </section>
        ) : filtered.length > 0 ? (
          <section className="history-grid" aria-label="Recent captures">
            {filtered.map((entry) => (
              <HistoryCard
                key={entry.id}
                entry={entry}
                onDeleted={(artifactId) => {
                  setEntries((current) => current.filter(({ id }) => id !== artifactId));
                }}
              />
            ))}
          </section>
        ) : null}
      </div>
    </main>
  );
}

export function HistoryCard({
  entry,
  onDeleted,
}: {
  entry: ArtifactSummary;
  onDeleted: (artifactId: string) => void;
}) {
  const [busy, setBusy] = useState<"restoring" | "editing" | "opening" | "revealing" | "saving" | "deleting" | null>(null);
  const [restored, setRestored] = useState(false);
  const [saved, setSaved] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [error, setError] = useState("");
  const feedbackTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const deleteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const recordingPermanentlySaved = entry.kind !== "screenshot"
    && Boolean(entry.saved_path)
    && !entry.missing;

  useEffect(() => () => {
    if (feedbackTimer.current) clearTimeout(feedbackTimer.current);
    if (deleteTimer.current) clearTimeout(deleteTimer.current);
  }, []);

  const restore = async () => {
    if (busy || entry.kind !== "screenshot") return;
    setBusy("restoring");
    setError("");
    try {
      await invoke("restore_history_artifact", { artifactId: entry.id });
      setRestored(true);
      if (feedbackTimer.current) clearTimeout(feedbackTimer.current);
      feedbackTimer.current = setTimeout(() => setRestored(false), 2_500);
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(null);
    }
  };

  const openRecording = async () => {
    if (busy || entry.kind === "screenshot" || entry.missing) return;
    setBusy("opening");
    setError("");
    try {
      await invoke("open_recording_editor", { artifactId: entry.id });
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(null);
    }
  };

  const editScreenshot = async () => {
    if (busy || entry.kind !== "screenshot") return;
    setBusy("editing");
    setError("");
    try {
      await invoke("restore_history_artifact", { artifactId: entry.id });
      await invoke("open_screenshot_editor", { artifactId: entry.id });
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(null);
    }
  };

  const revealRecording = async () => {
    if (busy || entry.kind === "screenshot" || entry.missing) return;
    setBusy("revealing");
    setError("");
    try {
      await invoke("reveal_recording_artifact", { artifactId: entry.id });
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(null);
    }
  };

  const saveRecording = async () => {
    if (busy || entry.kind === "screenshot" || entry.missing) return;
    setBusy("saving");
    setError("");
    try {
      await invoke("save_recording_artifact", { artifactId: entry.id });
      setSaved(true);
      if (feedbackTimer.current) clearTimeout(feedbackTimer.current);
      feedbackTimer.current = setTimeout(() => setSaved(false), 2_500);
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(null);
    }
  };

  const deleteFromHistory = async () => {
    if (busy) return;
    const requiresConfirmation = entry.kind === "screenshot" || !entry.missing;
    if (requiresConfirmation && !confirmingDelete) {
      setConfirmingDelete(true);
      if (deleteTimer.current) clearTimeout(deleteTimer.current);
      deleteTimer.current = setTimeout(() => setConfirmingDelete(false), 4_000);
      return;
    }

    setBusy("deleting");
    setError("");
    try {
      await invoke("delete_history_artifact", { artifactId: entry.id });
      onDeleted(entry.id);
    } catch (error) {
      setError(String(error));
      setBusy(null);
      setConfirmingDelete(false);
    }
  };

  const openCapture = async () => {
    if (busy) return;
    if (entry.kind === "screenshot") {
      await editScreenshot();
      return;
    }
    if (!entry.missing) {
      await openRecording();
    }
  };

  const previewDisabled = busy !== null || (entry.kind !== "screenshot" && entry.missing);
  const previewLabel = entry.kind === "screenshot"
    ? "Open screenshot in editor"
    : entry.missing
      ? undefined
      : `Open ${entry.kind === "gif" ? "GIF" : "video"} in editor`;

  return (
    <article className="history-card">
      <button
        type="button"
        className={[
          "history-image-wrap",
          entry.kind !== "screenshot" && entry.missing ? "history-image-missing" : "",
          previewDisabled ? "" : "history-image-open",
        ].filter(Boolean).join(" ")}
        disabled={previewDisabled}
        aria-label={previewLabel}
        onClick={() => void openCapture()}
      >
        <img
          src={entry.kind === "screenshot" ? entry.preview_url : entry.poster_url}
          alt={entry.kind === "screenshot" ? "Screenshot from capture history" : `${entry.kind === "gif" ? "GIF" : "Video"} recording poster`}
          loading="lazy"
          draggable={false}
        />
        {entry.kind !== "screenshot" && entry.missing && <span className="history-missing-label">File missing</span>}
      </button>
      <div className="history-card-body">
        <time dateTime={entry.created_at}>{formatHistoryDate(entry.created_at)}</time>
        <p>
          {entry.width} × {entry.height} · {formatFileSize(entry.size_bytes)}
          {entry.kind !== "screenshot" && <> · {formatRecordingTime(entry.duration_ms)}</>}
        </p>
        {entry.kind !== "screenshot" && entry.dropped_frames > 0 && <p className="history-recording-warning">{entry.dropped_frames.toLocaleString()} frame{entry.dropped_frames === 1 ? "" : "s"} dropped while recording</p>}
        <div className={[
          "history-actions",
          entry.kind === "screenshot" ? "history-screenshot-actions" : "history-recording-actions",
          entry.kind !== "screenshot" && entry.missing ? "history-missing-actions" : "",
        ].filter(Boolean).join(" ")}>
          {entry.kind === "screenshot" ? (
            <>
              <button
                type="button"
                className="history-edit"
                disabled={busy !== null}
                onClick={() => void editScreenshot()}
              >
                <EditIcon />{busy === "editing" ? "Opening…" : "Edit"}
              </button>
              <button
                type="button"
                className="history-reveal"
                title="Bring this screenshot back as a floating preview"
                disabled={busy !== null}
                onClick={() => void restore()}
              >
                {restored ? <><CheckIcon />Restored</> : <><RestoreIcon />{busy === "restoring" ? "Restoring…" : "Restore"}</>}
              </button>
            </>
          ) : !entry.missing ? (
            <>
              <button
                type="button"
                className="history-edit"
                disabled={busy !== null}
                onClick={() => void openRecording()}
              >
                <EditIcon />{busy === "opening" ? "Opening…" : "Edit"}
              </button>
              {recordingPermanentlySaved || saved ? (
                <button
                  type="button"
                  className="history-reveal"
                  disabled={busy !== null}
                  onClick={() => void revealRecording()}
                >
                  {busy === "revealing" ? "Showing…" : "Show in Folder"}
                </button>
              ) : (
                <button
                  type="button"
                  className="history-reveal"
                  title="Save a permanent copy to your Captures folder"
                  disabled={busy !== null}
                  onClick={() => void saveRecording()}
                >
                  {busy === "saving" ? "Saving…" : <><SaveIcon />Save file</>}
                </button>
              )}
            </>
          ) : null}
          <button
            type="button"
            className={confirmingDelete ? "history-delete history-delete-confirm" : "history-delete"}
            aria-label={entry.kind !== "screenshot" && entry.missing
              ? "Remove missing entry"
              : confirmingDelete
              ? "Confirm permanent deletion"
              : "Delete from History"}
            disabled={busy !== null}
            onClick={() => void deleteFromHistory()}
          >
            <TrashIcon />
            {entry.kind !== "screenshot" && entry.missing
              ? busy === "deleting" ? "Removing…" : "Remove entry"
              : confirmingDelete
                ? "Delete forever"
                : "Delete"}
          </button>
        </div>
        {error && <p className="history-card-error" role="alert">{error}</p>}
      </div>
    </article>
  );
}

function formatHistoryDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "Unknown date";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

function HistoryIcon() {
  return <svg viewBox="0 0 24 24"><path d="M3 12a9 9 0 1 0 3-6.7L3 8" /><path d="M3 3v5h5M12 7v5l3 2" /></svg>;
}

function RestoreIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 12a8 8 0 1 0 2.3-5.7L4 8" /><path d="M4 4v4h4" /></svg>;
}

function PauseResumeIcon({ paused }: { paused: boolean }) {
  return paused
    ? <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m8 5 11 7-11 7Z" /></svg>
    : <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14M16 5v14" /></svg>;
}

function RestartRecordingIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 11a8 8 0 1 1 2 5.3" /><path d="M4 5v6h6" /></svg>;
}

function WarningIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true">
    <path d="M10.29 4.86 1.82 19a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 4.86a2 2 0 0 0-3.42 0Z" />
    <path d="M12 9.5v5.2M12 17.6h.01" />
  </svg>;
}

function CaptureIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true">
    <path d="M9 4H7a3 3 0 0 0-3 3v2M15 4h2a3 3 0 0 1 3 3v2M20 15v2a3 3 0 0 1-3 3h-2M9 20H7a3 3 0 0 1-3-3v-2" />
    <path className="capture-icon-spark" d="M12 8.5c.4 1.8 1.7 3.1 3.5 3.5-1.8.4-3.1 1.7-3.5 3.5-.4-1.8-1.7-3.1-3.5-3.5 1.8-.4 3.1-1.7 3.5-3.5Z" />
  </svg>;
}

function SegmentedControlIndicator({ value }: { value: string }) {
  const indicatorRef = useRef<HTMLSpanElement>(null);

  useLayoutEffect(() => {
    const indicator = indicatorRef.current;
    const control = indicator?.parentElement;
    if (!indicator || !control) return;

    const update = () => {
      const activeButton = control.querySelector<HTMLElement>('button[aria-pressed="true"]');
      if (!activeButton) return;
      indicator.style.width = `${activeButton.offsetWidth}px`;
      indicator.style.transform = `translate3d(${activeButton.offsetLeft}px, 0, 0)`;
      indicator.classList.add("ready");
    };

    update();
    if (typeof ResizeObserver !== "function") return;
    const observer = new ResizeObserver(update);
    observer.observe(control);
    return () => observer.disconnect();
  }, [value]);

  return <span ref={indicatorRef} className="capture-segmented-indicator" aria-hidden="true" />;
}

function MicrophoneIcon({ muted }: { muted: boolean }) {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="9" y="3" width="6" height="11" rx="3" /><path d="M6 11a6 6 0 0 0 11.4 2.6M12 18v3M9 21h6" />{muted && <path d="m4 4 16 16" />}</svg>;
}

function HideControlsIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true">
    <path d="m2 2 20 20" />
    <path d="M6.7 6.7C4.9 8 3.7 9.7 3 12c1.7 4.1 5 7 9 7 1.8 0 3.5-.6 4.9-1.6" />
    <path d="M10.7 5.1A10.9 10.9 0 0 1 12 5c4 0 7.3 2.9 9 7-.3.8-.7 1.5-1.2 2.2" />
    <path d="M14.1 14.1a3 3 0 0 1-4.2-4.2" />
  </svg>;
}

function HudTooltip({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <span className="recording-tooltip">
      {children}
      <span role="tooltip">{label}</span>
    </span>
  );
}

type RecordingTargetMode = "region" | "window" | "display";
type RecordingRect = { x: number; y: number; width: number; height: number };
type RecordingRegionDrag = {
  mode: SelectionDragMode;
  origin: SelectionPoint;
  initial: RecordingRect;
};
type RecordingPanelPosition = { left: number; top: number };
type RecordingPanelDrag = { pointerId: number; offsetX: number; offsetY: number };

export function CaptureGuidance({
  mode,
  feedback = false,
  hidden = false,
}: {
  mode: CaptureMode;
  feedback?: boolean;
  /** Fully hide while the user is dragging out a region selection. */
  hidden?: boolean;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [cursorOver, setCursorOver] = useState(false);
  // Mount at opacity 0, then flip data-ready so entrance is the same opacity
  // transition used for hover ducking (no keyframe fill-mode fighting fade-out).
  const [ready, setReady] = useState(false);
  const title = mode === "display"
    ? "Click to capture this display"
    : mode === "window"
      ? "Select a window to continue"
      : feedback
        ? "Click and drag to select a region"
        : "Drag to select a region";
  const hint = mode === "region"
    ? "Shift for square · Esc to cancel"
    : "Esc to cancel";
  const faded = hidden || cursorOver;

  useEffect(() => {
    let inner = 0;
    const outer = requestAnimationFrame(() => {
      inner = requestAnimationFrame(() => setReady(true));
    });
    return () => {
      cancelAnimationFrame(outer);
      cancelAnimationFrame(inner);
    };
  }, []);

  // pointer-events: none so selection works through the label — track the
  // cursor against the label bounds and fade it out of the way when covered.
  useEffect(() => {
    const onPointerMove = (event: PointerEvent) => {
      const el = rootRef.current;
      if (!el) return;
      const bounds = el.getBoundingClientRect();
      setCursorOver((current) => {
        const over = isPointerOverCaptureGuidance(
          event.clientX,
          event.clientY,
          bounds,
          current,
        );
        return current === over ? current : over;
      });
    };
    const onPointerLeave = () => {
      setCursorOver((current) => (current ? false : current));
    };
    window.addEventListener("pointermove", onPointerMove, { passive: true });
    document.documentElement.addEventListener("pointerleave", onPointerLeave);
    window.addEventListener("blur", onPointerLeave);
    return () => {
      window.removeEventListener("pointermove", onPointerMove);
      document.documentElement.removeEventListener("pointerleave", onPointerLeave);
      window.removeEventListener("blur", onPointerLeave);
    };
  }, []);

  return (
    <div
      ref={rootRef}
      className={`capture-guidance${feedback ? " capture-guidance-feedback" : ""}`}
      data-ready={ready ? "true" : undefined}
      data-faded={faded ? "true" : undefined}
      role="status"
      aria-live="polite"
      aria-hidden={faded || undefined}
    >
      <strong>{title}</strong>
      <span>{hint}</span>
    </div>
  );
}

export function ScreenshotCountdown() {
  const [remaining, setRemaining] = useState<number | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [exiting, setExiting] = useState(false);
  const cancellingRef = useRef(false);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    const applyRemaining = (next: number) => {
      if (!active) return;
      setRemaining((current) => current === null ? next : Math.min(current, next));
    };
    void listen<{ remaining_seconds: number }>("screenshot-countdown", ({ payload }) => {
      applyRemaining(payload.remaining_seconds);
    }).then(async (dispose) => {
      if (!active) {
        dispose();
        return;
      }
      unlisten = dispose;
      const current = await invoke<{ remaining_seconds: number } | null>(
        "get_screenshot_countdown",
      );
      if (current) applyRemaining(current.remaining_seconds);
    }).catch(() => undefined);
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  const cancel = useCallback(async () => {
    if (cancellingRef.current) return;
    cancellingRef.current = true;
    setCancelling(true);
    setExiting(true);
    try {
      await invoke("cancel_screenshot_countdown");
      if (!prefersReducedMotion()) {
        await new Promise((resolve) => setTimeout(resolve, RECORDING_COUNTDOWN_FADE_OUT_MS));
      }
    } finally {
      cancellingRef.current = false;
      setCancelling(false);
    }
  }, []);

  useEffect(() => onCaptureEscape(() => {
    void cancel();
  }), [cancel]);

  return (
    <main
      className={`recording-countdown${exiting ? " exiting" : ""}`}
      aria-live="assertive"
    >
      <div className="recording-countdown-content">
        <span>Screenshot in</span>
        <strong>{remaining ?? "…"}</strong>
        <small>{cancelling ? "Cancelling…" : "Press Esc to cancel"}</small>
      </div>
    </main>
  );
}

export function RecordingCountdown() {
  const [snapshot, setSnapshot] = useState<RecordingSessionSnapshot | null>(null);
  const [remaining, setRemaining] = useState<number | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [exiting, setExiting] = useState(false);
  const cancellingRef = useRef(false);

  useEffect(() => {
    let active = true;
    const dispose: (() => void)[] = [];
    void Promise.all([
      listen<RecordingSessionSnapshot>("recording-state-changed", ({ payload }) => {
        if (!active) return;
        setSnapshot(payload);
        if (payload.state !== "countdown") {
          setExiting(true);
        }
      }),
      listen<{ session_id: string; remaining_seconds: number }>("recording-countdown", ({ payload }) => {
        if (!active) return;
        setRemaining(payload.remaining_seconds);
      }),
    ]).then((listeners) => {
      if (active) dispose.push(...listeners);
      else listeners.forEach((unlisten) => unlisten());
    }).catch(() => undefined);
    void invoke<RecordingSessionSnapshot | null>("get_recording_snapshot").then((current) => {
      if (active && current) {
        setSnapshot(current);
        if (current.state !== "countdown") setExiting(true);
      }
    });
    return () => {
      active = false;
      dispose.forEach((unlisten) => unlisten());
    };
  }, []);

  const cancel = useCallback(async () => {
    if (!snapshot || cancellingRef.current) return;
    cancellingRef.current = true;
    setCancelling(true);
    setExiting(true);
    try {
      await invoke("discard_recording", { sessionId: snapshot.id });
      if (!prefersReducedMotion()) {
        await new Promise((resolve) => setTimeout(resolve, RECORDING_COUNTDOWN_FADE_OUT_MS));
      }
    } finally {
      cancellingRef.current = false;
      setCancelling(false);
    }
  }, [snapshot]);

  useEffect(() => onCaptureEscape(() => {
    void cancel();
  }), [cancel]);

  const count = snapshot?.state === "countdown"
    ? remaining ?? snapshot.countdown_remaining_seconds ?? snapshot.options.countdown_seconds
    : remaining ?? 1;
  return (
    <main
      className={`recording-countdown${exiting ? " exiting" : ""}`}
      aria-live="assertive"
    >
      <div className="recording-countdown-content">
        <span>Recording starts in</span>
        <strong>{count}</strong>
        <small>{cancelling ? "Cancelling…" : "Press Esc to cancel"}</small>
      </div>
    </main>
  );
}

function recordingRegionIndicatorRect(): RecordingRect | null {
  const values = ["x", "y", "width", "height"].map((name) => Number(query(name)));
  if (values.some((value) => !Number.isFinite(value))) return null;
  const [x, y, width, height] = values;
  if (width < 1 || height < 1) return null;
  return {
    x: Math.max(0, x),
    y: Math.max(0, y),
    width,
    height,
  };
}

/**
 * Passive region boundary shown for the lifetime of a region recording.
 * The dim and frame stay outside the transparent hole, so the selected pixels
 * remain clean even on platforms that cannot exclude overlay windows.
 */
export function RecordingRegionIndicator() {
  const rect = useMemo(() => recordingRegionIndicatorRect(), []);
  useEffect(() => {
    if (!rect) return;
    let active = true;
    afterNextPaint(() => {
      if (active) {
        void invoke("reveal_recording_region_indicator");
      }
    });
    return () => {
      active = false;
    };
  }, [rect]);
  if (!rect) {
    return <main className="recording-region-indicator" aria-hidden="true" />;
  }
  return (
    <main className="recording-region-indicator" aria-hidden="true">
      <CaptureDim mode="region" hole={rect} bounds={{ width: 0, height: 0 }} />
      <div
        className="recording-region-indicator-frame"
        style={{ left: rect.x, top: rect.y, width: rect.width, height: rect.height }}
      />
    </main>
  );
}

export function RecordingSelector() {
  const [session, setSession] = useState<RecordingSelectionSession | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [actionMode, setActionMode] = useState<"screenshot" | "recording">("screenshot");
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [devicesLoading, setDevicesLoading] = useState(false);
  const [devicesLoaded, setDevicesLoaded] = useState(false);
  const [targetMode, setTargetMode] = useState<RecordingTargetMode>("region");
  const [region, setRegion] = useState<RecordingRect | null>(null);
  const [regionAspect, setRegionAspect] = useState<RegionAspectPreset>("free");
  const [panelPosition, setPanelPosition] = useState<RecordingPanelPosition | null>(null);
  const [panelDragging, setPanelDragging] = useState(false);
  const [selectedWindow, setSelectedWindow] = useState<string | null>(null);
  const [hoveredWindow, setHoveredWindow] = useState<string | null>(null);
  const [hoveredDisplay, setHoveredDisplay] = useState(false);
  const [fps, setFps] = useState(60);
  const [maxResolution, setMaxResolution] = useState<MaxResolution>("original");
  const [showCursor, setShowCursor] = useState(true);
  const [showClicks, setShowClicks] = useState(false);
  const [systemAudio, setSystemAudio] = useState(false);
  const [microphoneId, setMicrophoneId] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [switchingDisplay, setSwitchingDisplay] = useState(false);
  const [error, setError] = useState("");
  const [focusVisibleSessionId, setFocusVisibleSessionId] = useState<string | null>(null);
  const [controlsExcluded, setControlsExcluded] = useState<boolean | null>(null);
  const [regionSelecting, setRegionSelecting] = useState(false);
  const surfaceRef = useRef<HTMLElement>(null);
  const panelRef = useRef<HTMLElement>(null);
  const lastWindowPointerRef = useRef<SelectionPoint | null>(null);
  const windowHoverSurfaceRef = useRef<string | null>(null);
  const panelDragRef = useRef<RecordingPanelDrag | null>(null);
  const panelResizeFromRef = useRef<{ width: number; height: number } | null>(null);
  const panelResizeAnimationRef = useRef<Animation | null>(null);
  const regionDragRef = useRef<RecordingRegionDrag | null>(null);
  const pendingRegionPointRef = useRef<SelectionPoint | null>(null);
  const pendingRegionForceSquareRef = useRef(false);
  const regionAspectRef = useRef<RegionAspectPreset>("free");
  const regionFrameRef = useRef<number | null>(null);
  const settingsRef = useRef<AppSettings | null>(null);
  const sessionRef = useRef<RecordingSelectionSession | null>(null);
  const activeSessionIdRef = useRef<string | null>(null);
  const audioDevicesRequestIdRef = useRef(0);
  const revealingSessionIdRef = useRef<string | null>(null);
  const visibleSnapshotRef = useRef<string | null>(null);
  /** Set when region create or window pick should auto-confirm (preference). */
  const autoStartAfterSelectionRef = useRef(false);
  /** Latest start() for Enter. Auto-start calls `start` directly; the primary button is hidden. */
  const startCaptureRef = useRef<() => void>(() => undefined);

  const clearRegionDrag = useCallback(() => {
    if (regionFrameRef.current !== null) {
      window.cancelAnimationFrame(regionFrameRef.current);
      regionFrameRef.current = null;
    }
    regionDragRef.current = null;
    pendingRegionPointRef.current = null;
    pendingRegionForceSquareRef.current = false;
    setRegionSelecting(false);
  }, []);

  const applyRegionDrag = useCallback((
    point: SelectionPoint,
    forceSquare: boolean,
    surface: { width: number; height: number },
  ) => {
    const drag = regionDragRef.current;
    if (!drag) return;
    setRegion(dragSelectionRect(
      drag.mode,
      drag.origin,
      point,
      drag.initial,
      surface,
      {
        aspectRatio: parseAspectRatioPreset(regionAspectRef.current),
        forceSquare,
      },
    ));
  }, []);

  useEffect(() => {
    regionAspectRef.current = regionAspect;
  }, [regionAspect]);

  const surfaceBounds = useCallback(() => {
    const bounds = surfaceRef.current?.getBoundingClientRect();
    return {
      width: bounds?.width ?? 0,
      height: bounds?.height ?? 0,
    };
  }, []);

  /** Apply a new aspect preset: snap any settled region immediately, and re-run an in-progress drag. */
  const changeRegionAspect = useCallback((next: RegionAspectPreset) => {
    setRegionAspect(next);
    // Keep the ref current before any synchronous re-drag so pointer math uses the new ratio.
    regionAspectRef.current = next;
    const surface = surfaceBounds();
    const point = pendingRegionPointRef.current;
    if (regionDragRef.current && point) {
      applyRegionDrag(point, pendingRegionForceSquareRef.current, surface);
      return;
    }
    const aspect = parseAspectRatioPreset(next);
    if (aspect === null) return;
    setRegion((current) => {
      if (!current || !isCapturableSelection(current)) return current;
      return constrainSelectionToAspect(current, aspect, surface);
    });
  }, [applyRegionDrag, surfaceBounds]);

  // Re-apply the active region drag when Shift is pressed or released mid-gesture
  // so the marquee snaps between free/selected aspect and a square without waiting
  // for another pointer move.
  useEffect(() => {
    const onShift = (event: KeyboardEvent) => {
      if (event.key !== "Shift" || !regionDragRef.current) return;
      const point = pendingRegionPointRef.current;
      if (!point) return;
      const forceSquare = event.type === "keydown";
      pendingRegionForceSquareRef.current = forceSquare;
      applyRegionDrag(point, forceSquare, surfaceBounds());
    };
    window.addEventListener("keydown", onShift, true);
    window.addEventListener("keyup", onShift, true);
    return () => {
      window.removeEventListener("keydown", onShift, true);
      window.removeEventListener("keyup", onShift, true);
    };
  }, [applyRegionDrag, surfaceBounds]);

  const loadAudioDevices = useCallback(() => {
    if (
      devicesLoading
      || devicesLoaded
      || !sessionRef.current?.recording_capabilities.microphone
    ) return;
    const requestId = audioDevicesRequestIdRef.current + 1;
    audioDevicesRequestIdRef.current = requestId;
    setDevicesLoading(true);
    void invoke<AudioDevice[]>("list_recording_audio_devices")
      .then((audioDevices) => {
        if (audioDevicesRequestIdRef.current !== requestId) return;
        setDevices(audioDevices);
        setDevicesLoaded(true);
      })
      .catch(() => {
        if (audioDevicesRequestIdRef.current !== requestId) return;
        setDevices([]);
        setDevicesLoaded(true);
      })
      .finally(() => {
        if (audioDevicesRequestIdRef.current !== requestId) return;
        setDevicesLoading(false);
      });
  }, [devicesLoaded, devicesLoading]);

  const revealSelector = useCallback((selectionId: string, snapshotUrl: string) => {
    if (activeSessionIdRef.current !== selectionId) return;
    const revealKey = `${selectionId}:${snapshotUrl}`;
    if (
      revealingSessionIdRef.current === revealKey
      || visibleSnapshotRef.current === revealKey
    ) return;
    revealingSessionIdRef.current = revealKey;
    void invoke("show_recording_selector", { selectionId }).then(() => {
      let revealStarted = false;
      const finishReveal = () => {
        if (revealStarted || activeSessionIdRef.current !== selectionId) return;
        revealStarted = true;
        window.clearTimeout(fallbackTimer);
        void invoke("reveal_recording_selector", { selectionId })
          .then(() => {
            if (activeSessionIdRef.current === selectionId) {
              visibleSnapshotRef.current = revealKey;
              if (revealingSessionIdRef.current === revealKey) {
                revealingSessionIdRef.current = null;
              }
              setFocusVisibleSessionId(selectionId);
            }
          })
          .catch((error) => {
            if (revealingSessionIdRef.current === revealKey) {
              revealingSessionIdRef.current = null;
              setError(String(error));
            }
          });
      };
      afterNextPaint(finishReveal);
      // WebKit can suspend requestAnimationFrame while this preloaded window
      // is at near-zero opacity. Always reveal after a short deadline so the
      // backend cannot retain an invisible "capture in progress" selection.
      const fallbackTimer = window.setTimeout(
        finishReveal,
        RECORDING_SELECTOR_REVEAL_FALLBACK_MS,
      );
    }).catch((error) => {
      if (revealingSessionIdRef.current === revealKey) {
        revealingSessionIdRef.current = null;
        setError(String(error));
      }
    });
  }, []);

  const cancelSelection = useCallback((
    selection: RecordingSelectionSession,
    onCancelled?: () => void,
  ) => {
    if (activeSessionIdRef.current !== selection.id) return;
    activeSessionIdRef.current = null;
    sessionRef.current = null;
    revealingSessionIdRef.current = null;
    visibleSnapshotRef.current = null;
    setFocusVisibleSessionId(null);
    setSession(null);
    clearRegionDrag();
    panelDragRef.current = null;
    setPanelDragging(false);
    setStarting(false);
    setSwitchingDisplay(false);
    setError("");
    void invoke("cancel_recording_selection", { selectionId: selection.id })
      .then(() => onCancelled?.())
      .catch((error) => {
        // A new selector may already be active by the time a stale cancellation
        // fails. Never replace that newer session with the one being dismissed.
        if (activeSessionIdRef.current !== null) return;
        activeSessionIdRef.current = selection.id;
        sessionRef.current = selection;
        setSession(selection);
        setError(String(error));
        revealSelector(selection.id, freezeFrameRevealKey(selection));
      });
  }, [clearRegionDrag, revealSelector]);

  useEffect(() => {
    let active = true;
    const disposers: Array<() => void> = [];
    const applySelection = (selection: RecordingSelectionSession, currentSettings: AppSettings) => {
      // A newly-created selector asks for the pending session while also
      // subscribing to the ready event. Both can resolve with the same
      // selection; re-applying it would clear the fade after reveal.
      if (
        activeSessionIdRef.current === selection.id
        && sessionRef.current?.id === selection.id
      ) {
        const previous = sessionRef.current;
        const next = keepReadyWindowTargets(previous, selection);
        const snapshotChanged = previous.snapshot_url !== next.snapshot_url
          || previous.frozen !== next.frozen;
        const revealKey = `${next.id}:${freezeFrameRevealKey(next)}`;
        sessionRef.current = next;
        setSession(next);
        if (previous.initial_mode !== next.initial_mode) {
          setActionMode(next.initial_mode);
        }
        if (previous.initial_target !== next.initial_target) {
          setTargetMode(next.initial_target);
          setHoveredWindow(null);
          setHoveredDisplay(false);
          if (next.initial_target !== "window") {
            setSelectedWindow(null);
          }
        }
        const modeChanged = previous.initial_mode !== next.initial_mode
          || previous.initial_target !== next.initial_target;
        if (snapshotChanged) {
          visibleSnapshotRef.current = null;
          revealingSessionIdRef.current = null;
          setFocusVisibleSessionId(null);
        } else if (modeChanged && visibleSnapshotRef.current === revealKey) {
          visibleSnapshotRef.current = null;
          revealSelector(next.id, freezeFrameRevealKey(next));
        }
        setSwitchingDisplay(false);
        return;
      }
      activeSessionIdRef.current = selection.id;
      sessionRef.current = selection;
      revealingSessionIdRef.current = null;
      visibleSnapshotRef.current = null;
      audioDevicesRequestIdRef.current += 1;
      setDevices([]);
      setDevicesLoaded(false);
      setDevicesLoading(false);
      setFocusVisibleSessionId(null);
      setSession(selection);
      setActionMode(selection.initial_mode);
      setFps(currentSettings.recording.video_fps);
      setMaxResolution(currentSettings.recording.video_max_resolution);
      const capabilities = selection.recording_capabilities;
      setShowCursor(
        capabilities.cursor_control ? currentSettings.recording.show_cursor : false,
      );
      setShowClicks(
        capabilities.click_highlights ? currentSettings.recording.highlight_clicks : false,
      );
      setSystemAudio(
        capabilities.system_audio ? currentSettings.recording.capture_system_audio : false,
      );
      setMicrophoneId(
        capabilities.microphone ? currentSettings.recording.microphone_device_id : null,
      );
      setTargetMode(selection.initial_target);
      setSelectedWindow(null);
      setHoveredWindow(null);
      setHoveredDisplay(false);
      // Region starts empty so the user can draw anywhere (including mid-screen).
      // A pre-sized frame made intra-region create drags impossible (they moved the frame).
      setRegion(null);
      autoStartAfterSelectionRef.current = false;
      clearRegionDrag();
      lastWindowPointerRef.current = null;
      windowHoverSurfaceRef.current = null;
      panelDragRef.current = null;
      setPanelDragging(false);
      setPanelPosition(null);
      setStarting(false);
      setSwitchingDisplay(false);
      setError("");
    };
    const onSelectionReady = ({ payload }: { payload: RecordingSelectionSession }) => {
      if (!active) return;
      const cached = settingsRef.current;
      if (cached) {
        applySelection(payload, cached);
        void invoke<AppSettings>("get_settings").then((latestSettings) => {
          if (!active) return;
          settingsRef.current = latestSettings;
          setSettings(latestSettings);
        }).catch(() => undefined);
        return;
      }
      void invoke<AppSettings>("get_settings").then((latestSettings) => {
        if (!active) return;
        settingsRef.current = latestSettings;
        setSettings(latestSettings);
        applySelection(payload, latestSettings);
      }).catch(() => {
        if (!active) return;
        void invoke("cancel_recording_selection", { selectionId: payload.id });
      });
    };

    // Register for future selections, but do not wait for the Tauri event
    // bridge before loading the selection already prepared by the backend.
    // A newly-created macOS WebView can otherwise sit on a blank, click-through
    // surface while event registration is still pending.
    void listen<RecordingSelectionSession>("recording-selection-ready", onSelectionReady)
      .then((unlisten) => {
        if (active) {
          disposers.push(unlisten);
        } else {
          unlisten();
        }
      })
      .catch((error) => {
        if (active) setError(String(error));
      });

    void Promise.all([
      invoke<RecordingSelectionSession | null>("get_recording_selection"),
      invoke<AppSettings>("get_settings"),
      invoke<boolean>("recording_controls_are_excluded").catch(() => false),
    ])
      .then(([pending, loadedSettings, excluded]) => {
        if (!active) return;
        settingsRef.current = loadedSettings;
        setSettings(loadedSettings);
        setControlsExcluded(excluded);
        if (pending) {
          applySelection(pending, loadedSettings);
        }
      })
      .catch((error) => {
        if (active) setError(String(error));
      });

    void listen<AppSettings>("settings-changed", ({ payload }) => {
      if (!active) return;
      settingsRef.current = payload;
      setSettings(payload);
      // Keep the selector privacy line in sync when the include preference flips.
      void invoke<boolean>("recording_controls_are_excluded")
        .then((excluded) => {
          if (active) setControlsExcluded(excluded);
        })
        .catch(() => {
          if (active) setControlsExcluded(false);
        });
    }).then((unlisten) => {
      if (active) {
        disposers.push(unlisten);
      } else {
        unlisten();
      }
    }).catch(() => undefined);

    return () => {
      active = false;
      activeSessionIdRef.current = null;
      sessionRef.current = null;
      revealingSessionIdRef.current = null;
      visibleSnapshotRef.current = null;
      clearRegionDrag();
      disposers.forEach((dispose) => dispose());
    };
  }, [clearRegionDrag, revealSelector]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const currentSession = sessionRef.current;
      if (!currentSession) return;
      if (isCaptureEscapeKey(event)) {
        event.preventDefault();
        event.stopImmediatePropagation();
        cancelSelection(currentSession);
        return;
      }
      if (event.repeat || event.isComposing) {
        return;
      }
      const target = event.target;
      const typingInField = target instanceof Element
        && target.closest("input, textarea, select, [contenteditable]");
      const settings = settingsRef.current;
      if (settings && !typingInField) {
        const shortcutEvent = {
          code: event.code,
          ctrlKey: event.ctrlKey,
          shiftKey: event.shiftKey,
          altKey: event.altKey,
          metaKey: event.metaKey,
        };
        if (eventMatchesShortcut(shortcutEvent, settings.region_shortcut)) {
          event.preventDefault();
          setActionMode("screenshot");
          setTargetMode("region");
          setHoveredWindow(null);
          setHoveredDisplay(false);
          setSelectedWindow(null);
          return;
        }
        if (eventMatchesShortcut(shortcutEvent, settings.window_shortcut)) {
          event.preventDefault();
          setActionMode("screenshot");
          setTargetMode("window");
          setHoveredWindow(null);
          setHoveredDisplay(false);
          return;
        }
        if (eventMatchesShortcut(shortcutEvent, settings.display_shortcut)) {
          event.preventDefault();
          setActionMode("screenshot");
          setTargetMode("display");
          setHoveredWindow(null);
          setHoveredDisplay(false);
          setSelectedWindow(null);
          return;
        }
        if (eventMatchesShortcut(shortcutEvent, settings.recording.video_shortcut)) {
          event.preventDefault();
          setActionMode("recording");
          setTargetMode("region");
          setHoveredWindow(null);
          setHoveredDisplay(false);
          setSelectedWindow(null);
          return;
        }
        if (eventMatchesShortcut(shortcutEvent, settings.recording.window_shortcut)) {
          event.preventDefault();
          setActionMode("recording");
          setTargetMode("window");
          setHoveredWindow(null);
          setHoveredDisplay(false);
          return;
        }
        if (eventMatchesShortcut(shortcutEvent, settings.recording.display_shortcut)) {
          event.preventDefault();
          setActionMode("recording");
          setTargetMode("display");
          setHoveredWindow(null);
          setHoveredDisplay(false);
          setSelectedWindow(null);
          return;
        }
      }
      if (
        event.key !== "Enter"
        || event.defaultPrevented
        || event.altKey
        || event.ctrlKey
        || event.metaKey
        || event.shiftKey
      ) {
        return;
      }
      if (
        target instanceof Element
        && target.closest("input, textarea, select, [contenteditable], [role=\"combobox\"], [role=\"listbox\"]")
      ) {
        return;
      }
      // Mode/target segmented buttons are where Enter needs to confirm capture.
      // Leave Close, preference links, and other dedicated actions to native activation.
      if (target instanceof Element) {
        const focusedButton = target.closest("button");
        if (
          focusedButton
          && !focusedButton.classList.contains("capture-selector-primary")
          && !focusedButton.closest(".capture-action-switch, .recording-target-switch")
        ) {
          return;
        }
      }
      const primaryAction = panelRef.current?.querySelector<HTMLButtonElement>(
        ".capture-selector-primary:not(:disabled)",
      );
      if (!primaryAction) return;
      event.preventDefault();
      startCaptureRef.current();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [cancelSelection]);

  // A hidden WKWebView can defer image loading until its native window is
  // onscreen. Do not make the selector depend exclusively on the snapshot's
  // onLoad event or the window can remain hidden forever. The native surface is
  // transparent and click-through while this safety path waits for paint.
  const selectionId = session?.id;
  const snapshotUrl = session?.snapshot_url;
  const selectorFrozen = session?.frozen;
  useEffect(() => {
    if (!selectionId) return;
    const snapshotKey = selectorFrozen === false ? (snapshotUrl || "live") : (snapshotUrl ?? "");
    if (selectorFrozen === false) {
      revealSelector(selectionId, snapshotKey);
      return;
    }
    const timer = window.setTimeout(() => {
      revealSelector(selectionId, snapshotKey);
    }, 120);
    return () => window.clearTimeout(timer);
  }, [selectionId, snapshotUrl, selectorFrozen, revealSelector]);

  useEffect(() => {
    if (!session?.id) return;
    const cursorClass = `capture-selector-${targetMode}`;
    document.documentElement.classList.add(cursorClass);
    return () => document.documentElement.classList.remove(cursorClass);
  }, [session?.id, targetMode]);

  const applyWindowHoverAt = useCallback((point: SelectionPoint) => {
    const current = sessionRef.current;
    if (!current) return;
    lastWindowPointerRef.current = point;
    const hover = windowPointerHoverAtPoint(
      current.windows,
      current.shell_chrome ?? [],
      point,
      current.display,
      Math.max(current.window_coordinate_scale || 1, 1),
      current.windows_ready,
    );
    setHoveredWindow(hover.windowId);
    setHoveredDisplay(hover.display);
  }, []);

  useEffect(() => {
    if (targetMode !== "window" || !session?.id) return;
    const surfaceKey = `${session.id}:${session.display.id}`;
    if (windowHoverSurfaceRef.current !== surfaceKey) {
      lastWindowPointerRef.current = null;
      windowHoverSurfaceRef.current = surfaceKey;
    }
    const existing = lastWindowPointerRef.current;
    if (existing) {
      applyWindowHoverAt(existing);
      return;
    }
    let cancelled = false;
    void requestCapturePointerPosition().then((pointer) => {
      if (cancelled || lastWindowPointerRef.current || !pointer?.inside) return;
      applyWindowHoverAt({ x: pointer.x, y: pointer.y });
    });
    return () => {
      cancelled = true;
    };
  }, [
    applyWindowHoverAt,
    session?.id,
    session?.display.id,
    session?.windows,
    session?.windows_ready,
    targetMode,
    focusVisibleSessionId,
  ]);

  useEffect(() => {
    if (!session?.id || focusVisibleSessionId !== session.id) return;
    void invoke("sync_selector_cursor", { selectionId: session.id, mode: targetMode });
  }, [focusVisibleSessionId, session?.id, targetMode]);

  useEffect(() => {
    if (
      !session?.id
      || actionMode !== "recording"
      || !session.recording_capabilities.microphone
    ) return;
    const timer = window.setTimeout(loadAudioDevices, 0);
    return () => window.clearTimeout(timer);
  }, [
    actionMode,
    loadAudioDevices,
    session?.id,
    session?.recording_capabilities.microphone,
  ]);

  useLayoutEffect(() => {
    const from = panelResizeFromRef.current;
    panelResizeFromRef.current = null;
    const panel = panelRef.current;
    if (!from || !panel) return;

    panelResizeAnimationRef.current?.cancel();
    panelResizeAnimationRef.current = null;
    panel.removeAttribute("data-resizing");

    const to = panel.getBoundingClientRect();
    if (
      prefersReducedMotion()
      || typeof panel.animate !== "function"
      || (Math.abs(from.width - to.width) < 0.5 && Math.abs(from.height - to.height) < 0.5)
    ) {
      return;
    }

    panel.dataset.resizing = "true";
    const animation = panel.animate([
      { width: `${from.width}px`, height: `${from.height}px` },
      { width: `${to.width}px`, height: `${to.height}px` },
    ], {
      duration: 280,
      easing: "cubic-bezier(.2,.8,.2,1)",
    });
    panelResizeAnimationRef.current = animation;

    const settle = () => {
      if (panelResizeAnimationRef.current !== animation) return;
      panelResizeAnimationRef.current = null;
      panel.removeAttribute("data-resizing");
    };
    animation.addEventListener("finish", settle, { once: true });
    animation.addEventListener("cancel", settle, { once: true });
  }, [actionMode]);

  useEffect(() => () => {
    panelResizeAnimationRef.current?.cancel();
  }, []);

  // Keep hooks above the idle early-return so session load never changes hook order.
  const overlaySize = session
    ? displayOverlaySize(session.display, session.window_coordinate_scale)
    : { width: 0, height: 0 };
  const surfaceSize = useElementCssSize(surfaceRef, overlaySize);

  const canStartSelection = Boolean(session) && !switchingDisplay && (
    targetMode === "display"
    || (targetMode === "window" && Boolean(selectedWindow))
    || (targetMode === "region" && Boolean(region && region.width >= 2 && region.height >= 2))
  );

  const selectedTarget = useCallback((): RecordingTarget | null => {
    const current = sessionRef.current;
    if (!current) return null;
    if (targetMode === "display") {
      return { type: "display", display_id: current.display.id };
    }
    if (targetMode === "window" && selectedWindow) {
      return { type: "window", window_id: selectedWindow };
    }
    if (region) {
      return {
        type: "region",
        display_id: current.display.id,
        rect: roundRecordingRect(region, surfaceSize.width, surfaceSize.height),
      };
    }
    return null;
  }, [region, selectedWindow, surfaceSize.height, surfaceSize.width, targetMode]);

  const start = useCallback(async () => {
    const currentSession = sessionRef.current;
    const currentSettings = settingsRef.current;
    if (
      !currentSession
      || !currentSettings
      || !canStartSelection
      || starting
      || switchingDisplay
    ) return;
    const target = selectedTarget();
    if (!target) return;
    setStarting(true);
    setError("");
    if (actionMode === "screenshot") {
      try {
        await invoke("capture_selection_screenshot", {
          request: { selection_id: currentSession.id, target },
        });
        activeSessionIdRef.current = null;
        revealingSessionIdRef.current = null;
        setSession(null);
        clearRegionDrag();
      } catch (error) {
        const message = String(error);
        if (message.includes("screenshot cancelled")) {
          activeSessionIdRef.current = null;
          revealingSessionIdRef.current = null;
          setSession(null);
          clearRegionDrag();
          return;
        }
        setError(message);
        setStarting(false);
      }
      return;
    }
    const capabilities = currentSession.recording_capabilities;
    const options: RecordingOptions = {
      kind: "video",
      target,
      frames_per_second: fps,
      max_resolution: maxResolution,
      countdown_seconds: currentSettings.recording.countdown_seconds,
      show_cursor: capabilities.cursor_control && showCursor,
      highlight_clicks: capabilities.click_highlights && showClicks,
      show_keystrokes: currentSettings.recording.show_keystrokes,
      audio: {
        capture_system_audio: capabilities.system_audio && systemAudio,
        microphone_device_id: capabilities.microphone ? microphoneId : null,
        mono_output: currentSettings.recording.mono_audio,
        system_volume_percent: 100,
        microphone_volume_percent: 100,
        microphone_muted: false,
      },
      gif: {
        max_width: currentSettings.recording.gif_max_width,
        max_colors: currentSettings.recording.gif_max_colors,
        optimize: true,
      },
    };
    try {
      await invoke("start_recording", {
        request: { selection_id: currentSession.id, options },
      });
      activeSessionIdRef.current = null;
      revealingSessionIdRef.current = null;
      setSession(null);
      clearRegionDrag();
    } catch (error) {
      setError(String(error));
      setStarting(false);
    }
  }, [
    actionMode,
    canStartSelection,
    clearRegionDrag,
    fps,
    maxResolution,
    microphoneId,
    selectedTarget,
    showClicks,
    showCursor,
    starting,
    switchingDisplay,
    systemAudio,
  ]);

  useLayoutEffect(() => {
    startCaptureRef.current = () => {
      void start();
    };
  }, [start]);

  // Preference: selecting any target starts the capture immediately.
  useEffect(() => {
    if (!autoStartAfterSelectionRef.current || !canStartSelection || starting || switchingDisplay) {
      return;
    }
    autoStartAfterSelectionRef.current = false;
    void start();
  }, [
    canStartSelection,
    start,
    starting,
    switchingDisplay,
    region,
    selectedWindow,
    targetMode,
    actionMode,
  ]);

  if (!session || !settings) {
    return <main className="recording-selector-idle" aria-hidden="true" />;
  }

  const point = (event: React.PointerEvent): SelectionPoint => {
    const bounds = surfaceRef.current?.getBoundingClientRect();
    return {
      x: Math.max(0, Math.min(bounds?.width ?? 0, event.clientX - (bounds?.left ?? 0))),
      y: Math.max(0, Math.min(bounds?.height ?? 0, event.clientY - (bounds?.top ?? 0))),
    };
  };
  const windowAtPointer = (event: React.PointerEvent) => frontmostCaptureTargetAtPoint(
    session.windows,
    session.shell_chrome ?? [],
    point(event),
    session.display,
    Math.max(session.window_coordinate_scale || 1, 1),
  );
  const onPointerDown = (event: React.PointerEvent) => {
    if ((event.target as Element).closest(".recording-selector-panel")) return;
    if (targetMode === "window") {
      const hit = windowAtPointer(event);
      if (hit?.kind === "window") {
        setSelectedWindow(hit.target.id);
        setHoveredWindow(hit.target.id);
        setHoveredDisplay(false);
        if (settingsRef.current?.auto_start_on_selection) {
          autoStartAfterSelectionRef.current = true;
        }
        return;
      }
      if (hit?.kind === "chrome" || windowListingIsReady(session.windows_ready)) {
        setSelectedWindow(null);
        setHoveredWindow(null);
        setHoveredDisplay(false);
        setTargetMode("display");
        if (settingsRef.current?.auto_start_on_selection) {
          autoStartAfterSelectionRef.current = true;
        }
      }
      return;
    }
    if (targetMode !== "region") return;
    event.preventDefault();
    const start = point(event);
    const target = event.target as Element;
    const handle = target.closest<HTMLElement>("[data-selection-handle]")?.dataset.selectionHandle as SelectionDragMode | undefined;
    const mode: SelectionDragMode = handle
      ?? (target.closest(".recording-selection-frame") ? "move" : "create");
    if (mode !== "create" && !region) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    clearRegionDrag();
    pendingRegionForceSquareRef.current = event.shiftKey;
    pendingRegionPointRef.current = start;
    regionDragRef.current = {
      mode,
      origin: start,
      initial: region ?? { x: start.x, y: start.y, width: 0, height: 0 },
    };
    if (mode === "create") {
      setRegionSelecting(true);
      setRegion({ x: start.x, y: start.y, width: 0, height: 0 });
    }
  };
  const onPointerMove = (event: React.PointerEvent) => {
    if (targetMode === "window") {
      if ((event.target as Element).closest(".recording-selector-panel")) return;
      applyWindowHoverAt(point(event));
      return;
    }
    if (!regionDragRef.current || targetMode !== "region") return;
    event.preventDefault();
    pendingRegionPointRef.current = point(event);
    pendingRegionForceSquareRef.current = event.shiftKey;
    if (regionFrameRef.current !== null) return;
    regionFrameRef.current = window.requestAnimationFrame(() => {
      regionFrameRef.current = null;
      const current = pendingRegionPointRef.current;
      if (!current || !regionDragRef.current) return;
      applyRegionDrag(current, pendingRegionForceSquareRef.current, surfaceSize);
    });
  };
  const onPointerUp = (event: React.PointerEvent) => {
    const drag = regionDragRef.current;
    const current = drag ? point(event) : null;
    const forceSquare = event.shiftKey || pendingRegionForceSquareRef.current;
    let finishedCreate: ReturnType<typeof dragSelectionRect> | null = null;
    if (drag && current) {
      const next = dragSelectionRect(
        drag.mode,
        drag.origin,
        current,
        drag.initial,
        surfaceSize,
        {
          aspectRatio: parseAspectRatioPreset(regionAspectRef.current),
          forceSquare,
        },
      );
      setRegion(next);
      if (drag.mode === "create") finishedCreate = next;
    }
    if (
      typeof event.currentTarget.hasPointerCapture === "function"
      && event.currentTarget.hasPointerCapture(event.pointerId)
    ) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    clearRegionDrag();
    // Auto-start only after a create drag finishes — move/resize stay adjust-only.
    if (
      finishedCreate
      && settingsRef.current?.auto_start_on_selection
      && isCapturableSelection(finishedCreate)
    ) {
      autoStartAfterSelectionRef.current = true;
    }
  };
  const onPointerCancel = (event: React.PointerEvent) => {
    const drag = regionDragRef.current;
    if (
      typeof event.currentTarget.hasPointerCapture === "function"
      && event.currentTarget.hasPointerCapture(event.pointerId)
    ) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (drag) {
      setRegion(drag.initial.width > 0 && drag.initial.height > 0 ? drag.initial : null);
    }
    clearRegionDrag();
  };

  const selectableWindows = frontToBackWindows(session.windows);
  const windowLayouts = selectableWindows.map((window, index) => {
    const scale = Math.max(session.window_coordinate_scale || 1, 1);
    return {
      window,
      left: (window.x - session.display.x) / scale,
      top: (window.y - session.display.y) / scale,
      width: window.width / scale,
      height: window.height / scale,
      cornerRadius: window.corner_radius ?? session.window_corner_radius,
      zIndex: selectableWindows.length - index,
    };
  });
  const activeWindow = hoveredWindow ?? selectedWindow;
  const activeWindowLayout = windowLayouts.find(({ window }) => window.id === activeWindow);
  const displayOptions = (session.displays.length > 0 ? session.displays : [session.display])
    .map((display, index) => ({
      value: display.id,
      label: display.name.trim() || `Display ${index + 1}`,
      description: `${display.width} × ${display.height}${display.is_primary ? " · Primary" : ""}`,
    }));
  const selectedRect = targetMode === "display"
    ? { x: 0, y: 0, width: surfaceSize.width, height: surfaceSize.height }
    : targetMode === "window"
      ? activeWindowLayout ? {
          x: activeWindowLayout.left,
          y: activeWindowLayout.top,
          width: activeWindowLayout.width,
          height: activeWindowLayout.height,
        } : null
      : region;
  const activeWindowCornerRadius = activeWindowLayout?.cornerRadius ?? session.window_corner_radius;
  const displayCornerRadius = Math.max(0, session.display_corner_radius ?? 0);
  const canStart = canStartSelection;

  const switchDisplay = async (displayId: string) => {
    if (displayId === session.display.id || switchingDisplay || starting) return;
    setSwitchingDisplay(true);
    setError("");
    try {
      const next = await invoke<RecordingSelectionSession | null>("select_capture_display", {
        selectionId: session.id,
        displayId,
      });
      if (!next?.id || !next.display?.id) {
        setError("Could not switch displays.");
        return;
      }
      if (activeSessionIdRef.current !== next.id) return;
      sessionRef.current = next;
      setSession(next);
      setRegion(null);
      lastWindowPointerRef.current = null;
      windowHoverSurfaceRef.current = null;
      autoStartAfterSelectionRef.current = false;
      setSelectedWindow(null);
      setHoveredWindow(null);
      setHoveredDisplay(false);
      if (targetMode === "display" && settingsRef.current?.auto_start_on_selection) {
        autoStartAfterSelectionRef.current = true;
      }
    } catch (error) {
      setError(String(error));
    } finally {
      setSwitchingDisplay(false);
    }
  };

  const beginPanelDrag = (event: React.PointerEvent<HTMLElement>) => {
    event.stopPropagation();
    if ((event.target as Element).closest("button, input, label, a, .custom-select")) return;
    const panel = panelRef.current;
    if (!panel) return;
    event.preventDefault();
    const bounds = panel.getBoundingClientRect();
    panelDragRef.current = {
      pointerId: event.pointerId,
      offsetX: event.clientX - bounds.left,
      offsetY: event.clientY - bounds.top,
    };
    setPanelPosition({ left: bounds.left, top: bounds.top });
    setPanelDragging(true);
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const movePanel = (event: React.PointerEvent<HTMLElement>) => {
    const drag = panelDragRef.current;
    const panel = panelRef.current;
    if (!drag || !panel || drag.pointerId !== event.pointerId) return;
    event.preventDefault();
    event.stopPropagation();
    const margin = 8;
    const maxLeft = Math.max(margin, window.innerWidth - panel.offsetWidth - margin);
    const maxTop = Math.max(margin, window.innerHeight - panel.offsetHeight - margin);
    setPanelPosition({
      left: Math.min(maxLeft, Math.max(margin, event.clientX - drag.offsetX)),
      top: Math.min(maxTop, Math.max(margin, event.clientY - drag.offsetY)),
    });
  };
  const endPanelDrag = (event: React.PointerEvent<HTMLElement>) => {
    if (panelDragRef.current?.pointerId !== event.pointerId) return;
    panelDragRef.current = null;
    setPanelDragging(false);
    if (
      typeof event.currentTarget.hasPointerCapture === "function"
      && event.currentTarget.hasPointerCapture(event.pointerId)
    ) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };
  const switchActionMode = (mode: "screenshot" | "recording") => {
    if (mode === actionMode) return;
    const panel = panelRef.current;
    if (panel) {
      const bounds = panel.getBoundingClientRect();
      panelResizeFromRef.current = { width: bounds.width, height: bounds.height };
    }
    setActionMode(mode);
  };
  const openCapturePreference = (target: string) => {
    // Cancelling destroys this selector WebView before its IPC promise settles,
    // so finish opening Preferences while the caller is still alive.
    void invoke("open_preferences", { target })
      .then(() => cancelSelection(session))
      .catch((error) => setError(String(error)));
  };
  const retryingAutoStart = settings.auto_start_on_selection && Boolean(error);
  const primaryActionLabel = starting
    ? actionMode === "screenshot" ? "Capturing…" : "Starting…"
    : switchingDisplay ? "Switching…"
    : retryingAutoStart ? actionMode === "screenshot" ? "Retry capture" : "Retry recording"
    : actionMode === "screenshot" ? "Capture" : "Start recording";
  const primaryActionAriaLabel = retryingAutoStart
    ? actionMode === "screenshot" ? "Retry capture" : "Retry recording"
    : actionMode === "screenshot" ? "Take screenshot" : "Start recording";

  return (
    <main
      ref={surfaceRef}
      className={`recording-selector recording-target-${targetMode}${focusVisibleSessionId === session.id ? " recording-focus-visible" : ""}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
      onPointerEnter={(event) => {
        if (targetMode !== "window") return;
        if ((event.target as Element).closest(".recording-selector-panel")) return;
        applyWindowHoverAt(point(event));
      }}
      onDragStart={(event) => event.preventDefault()}
    >
      {sessionShowsFreezeFrame(session) && session.snapshot_url ? (
        <img
          className="recording-selector-snapshot"
          src={session.snapshot_url}
          alt=""
          draggable={false}
          onLoad={() => revealSelector(session.id, freezeFrameRevealKey(session))}
          onError={() => {
            setError("The frozen preview could not load. You can still select from the live desktop.");
            revealSelector(session.id, freezeFrameRevealKey(session));
          }}
        />
      ) : null}
      <CaptureDim
        mode={targetMode}
        hole={targetMode === "display" || hoveredDisplay ? null : selectedRect}
        bounds={surfaceSize}
        dimWithoutHole={targetMode === "window" && !hoveredDisplay}
        windowCornerRadius={activeWindowCornerRadius}
      />
      {(targetMode === "display" || hoveredDisplay) && <>
        <div
          className="recording-display-outline"
          aria-hidden="true"
          style={displayCornerRadius > 0 ? { borderRadius: displayCornerRadius } : undefined}
        />
        {targetMode === "display" && (
        <div className="recording-display-identity" aria-live="polite">
          <span className="recording-display-icon" aria-hidden="true">
            <CaptureTargetIcon mode="display" />
          </span>
          <strong>{session.display.name || "Display"}</strong>
          <span>
            {session.display.width} × {session.display.height}
            {actionMode === "recording" ? ` · ${fps} FPS` : ""}
          </span>
        </div>
        )}
      </>}
      {targetMode === "region" && (
        <CaptureGuidance mode="region" hidden={regionSelecting} />
      )}
      {targetMode === "window" && !selectedWindow && (
        <CaptureGuidance mode={hoveredDisplay ? "display" : "window"} />
      )}
      {targetMode === "region" && selectedRect && selectedRect.width > 0 && selectedRect.height > 0 && (
        <div
          className={`recording-selection-frame recording-selection-${targetMode}${targetMode === "region" ? " movable" : ""}`}
          style={{
            left: selectedRect.x,
            top: selectedRect.y,
            width: selectedRect.width,
            height: selectedRect.height,
          }}
        >
          <span
            className="selection-dimensions"
            data-screen-edge={selectedRect.y < 30 ? "top" : undefined}
          >
            {Math.round(selectedRect.width)} × {Math.round(selectedRect.height)}
          </span>
          {targetMode === "region" && <>
            <i className="handle nw" data-selection-handle="nw" />
            <i className="handle ne" data-selection-handle="ne" />
            <i className="handle sw" data-selection-handle="sw" />
            <i className="handle se" data-selection-handle="se" />
          </>}
        </div>
      )}
      {targetMode === "window" && (
        <div className="recording-window-targets">
          {windowLayouts.map(({ window, left, top, width, height, cornerRadius, zIndex }) => (
            <button
              key={window.id}
              type="button"
              className={`recording-window-target${selectedWindow === window.id ? " selected" : ""}${hoveredWindow === window.id ? " hovered" : ""}`}
              style={{
                left,
                top,
                width,
                height,
                zIndex,
                borderRadius: cornerRadius,
              }}
              aria-label={`Select ${window.title || "window"}`}
            >
              <span>{window.title || window.app_name || "Window"}</span>
            </button>
          ))}
        </div>
      )}

      <section
        ref={panelRef}
        className={`recording-selector-panel on-media${panelDragging ? " dragging" : ""}`}
        style={panelPosition ? {
          left: panelPosition.left,
          top: panelPosition.top,
          bottom: "auto",
          transform: "none",
        } : undefined}
        onPointerDown={beginPanelDrag}
        onPointerMove={movePanel}
        onPointerUp={endPanelDrag}
        onPointerCancel={endPanelDrag}
      >
        <div className="recording-panel-top">
          <button
            className="capture-selector-close"
            type="button"
            aria-label="Close capture controls"
            onClick={() => cancelSelection(session)}
          ><CloseIcon /></button>
          <div
            className="capture-action-switch"
            role="group"
            aria-label="Capture type"
            data-active={actionMode}
          >
            <SegmentedControlIndicator value={actionMode} />
            <button
              type="button"
              className={actionMode === "screenshot" ? "active" : ""}
              aria-pressed={actionMode === "screenshot"}
              onClick={() => switchActionMode("screenshot")}
            ><CaptureIcon />Screenshot</button>
            <button
              type="button"
              className={actionMode === "recording" ? "active" : ""}
              aria-pressed={actionMode === "recording"}
              disabled={!session.recording_available}
              title={session.recording_available ? undefined : "Screen recording is not available on this platform"}
              onClick={() => switchActionMode("recording")}
            ><span className="capture-record-dot" aria-hidden="true" />Record</button>
          </div>
          <span className="capture-selector-divider" aria-hidden="true" />
          <div
            className="recording-target-switch"
            role="group"
            aria-label="Capture target"
            data-active={targetMode}
          >
            <SegmentedControlIndicator value={targetMode} />
            {(["region", "window", "display"] as const).map((mode) => (
              <button
                key={mode}
                type="button"
                className={targetMode === mode ? "active" : ""}
                aria-pressed={targetMode === mode}
                disabled={mode === "window" && windowLayouts.length === 0 && windowListingIsReady(session.windows_ready)}
                title={mode === "window" && windowLayouts.length === 0
                  ? "Window capture is not available in this desktop session"
                  : undefined}
                onClick={() => {
                  setTargetMode(mode);
                  setHoveredWindow(null);
                  setHoveredDisplay(false);
                  if (mode === "display" && settingsRef.current?.auto_start_on_selection) {
                    if (targetMode === "display") {
                      void start();
                    } else {
                      autoStartAfterSelectionRef.current = true;
                    }
                  } else {
                    // Region and window still need a drawn/picked target.
                    // Drop a pending full-screen auto-start so switching away
                    // from Full screen cannot start and dismiss the menu.
                    autoStartAfterSelectionRef.current = false;
                  }
                }}
              >
                <CaptureTargetIcon mode={mode} />
                <span>{mode === "display" ? "Full screen" : mode[0].toUpperCase() + mode.slice(1)}</span>
              </button>
            ))}
          </div>
          {targetMode === "display" && (
            <div className="recording-display-picker">
              <CustomSelect
                value={session.display.id}
                options={displayOptions}
                ariaLabel="Display"
                disabled={switchingDisplay || starting || displayOptions.length < 2}
                onChange={(displayId) => void switchDisplay(displayId)}
              />
            </div>
          )}
          {targetMode === "region" && (
            <div className="recording-region-aspect-picker">
              <span className="recording-region-aspect-label">Aspect</span>
              <CustomSelect
                value={regionAspect}
                options={REGION_ASPECT_PRESETS.map((preset) => ({
                  value: preset.value,
                  label: preset.label,
                }))}
                ariaLabel="Region aspect ratio"
                disabled={starting}
                onChange={(value) => changeRegionAspect(value as RegionAspectPreset)}
              />
            </div>
          )}
          <button
            className={`recording-start capture-selector-primary capture-selector-primary-${actionMode}`}
            type="button"
            aria-label={primaryActionAriaLabel}
            aria-keyshortcuts="Enter"
            disabled={!canStart || starting}
            hidden={settings.auto_start_on_selection && !starting && !error}
            onClick={() => void start()}
          >
            {actionMode === "screenshot"
              ? <CaptureIcon />
              : <span className="capture-record-dot" aria-hidden="true" />}
            {primaryActionLabel}
          </button>
        </div>
        {actionMode === "recording" && (
          <div className="recording-options-row">
            <div className="recording-field"><span>FPS</span>
              <CustomSelect
                value={String(fps)}
                ariaLabel="Frames per second"
                options={[60, 30, 15].map((value) => ({ value: String(value), label: String(value) }))}
                onChange={(value) => setFps(Number(value))}
              />
            </div>
            <div className="recording-field"><span>Max resolution</span>
              <CustomSelect
                value={maxResolution}
                ariaLabel="Maximum resolution"
                options={[
                  { value: "original", label: "Original" },
                  { value: "p1080", label: "1080p" },
                  { value: "p720", label: "720p" },
                ]}
                onChange={(value) => setMaxResolution(value as MaxResolution)}
              />
            </div>
            <div className="recording-field"><span>Show cursor</span>
              <label
                className="recording-toggle"
                title={session.recording_capabilities.cursor_control
                  ? undefined
                  : "Cursor capture is unavailable in this desktop session"}
              >
                <input
                  aria-label="Show cursor"
                  type="checkbox"
                  checked={showCursor}
                  disabled={!session.recording_capabilities.cursor_control}
                  onChange={(event) => {
                    setShowCursor(event.target.checked);
                    if (!event.target.checked) setShowClicks(false);
                  }}
                />
                <span className="recording-switch" aria-hidden="true" />
                <span>{session.recording_capabilities.cursor_control
                  ? showCursor ? "On" : "Off"
                  : "Unavailable"}</span>
              </label>
            </div>
            <div className="recording-field"><span>Show clicks</span>
              <label
                className="recording-toggle"
                title={session.recording_capabilities.click_highlights
                  ? undefined
                  : "Click highlights are unavailable in this desktop session"}
              >
                <input
                  aria-label="Show clicks"
                  type="checkbox"
                  checked={showClicks}
                  disabled={!session.recording_capabilities.click_highlights}
                  onChange={(event) => {
                    setShowClicks(event.target.checked);
                    if (event.target.checked) setShowCursor(true);
                  }}
                />
                <span className="recording-switch" aria-hidden="true" />
                <span>{session.recording_capabilities.click_highlights
                  ? showClicks ? "On" : "Off"
                  : "Unavailable"}</span>
              </label>
            </div>
            <div className="recording-field"><span>Desktop audio</span>
              <label
                className="recording-toggle"
                title={session.recording_capabilities.system_audio
                  ? undefined
                  : "Desktop audio recording is unavailable in this desktop session"}
              >
                <input
                  aria-label="Record desktop audio"
                  type="checkbox"
                  checked={systemAudio}
                  disabled={!session.recording_capabilities.system_audio}
                  onChange={(event) => setSystemAudio(event.target.checked)}
                />
                <span className="recording-switch" aria-hidden="true" />
                <span>{session.recording_capabilities.system_audio
                  ? systemAudio ? "On" : "Off"
                  : "Unavailable"}</span>
              </label>
            </div>
            <div className="recording-field recording-microphone-field"><span>Microphone</span>
              <CustomSelect
                value={microphoneId ?? "off"}
                disabled={!session.recording_capabilities.microphone || devicesLoading}
                onOpen={loadAudioDevices}
                ariaLabel="Microphone"
                options={[
                  {
                    value: "off",
                    label: session.recording_capabilities.microphone ? "Off" : "Unavailable",
                  },
                  ...(devicesLoading ? [{ value: "__loading", label: "Loading microphones…", disabled: true }] : []),
                  ...(microphoneId && !devices.some((device) => device.id === microphoneId)
                    ? [{ value: microphoneId, label: devicesLoading ? "Loading microphone…" : "Selected microphone" }]
                    : []),
                  ...devices.map((device) => ({ value: device.id, label: device.name })),
                ]}
                onChange={(value) => setMicrophoneId(value === "off" ? null : value)}
              />
            </div>
          </div>
        )}
        <p className="capture-selector-note">
          <CaptureSelectorVisibilityNote
            canExcludeControls={session.recording_capabilities.can_exclude_controls}
            controlsExcluded={
              controlsExcluded ?? session.recording_capabilities.controls_excluded
            }
            actionMode={actionMode}
            onOpenPreference={() => openCapturePreference(RECORDING_CONTROLS_PREFERENCE_TARGET)}
          />
          {settings.auto_start_on_selection
            ? <>
              <span aria-hidden="true">·</span>
              <CapturePreferenceLink
                onClick={() => openCapturePreference(AUTO_START_PREFERENCE_TARGET)}
              >
                Auto-capture is on. Selecting a target starts immediately.
              </CapturePreferenceLink>
            </>
            : <>
              <span aria-hidden="true">·</span>
              Press <kbd>Enter</kbd> to confirm
            </>}
        </p>
        {error && <p className="recording-selector-error" role="alert">{error}</p>}
      </section>
    </main>
  );
}

function CaptureTargetIcon({ mode }: { mode: RecordingTargetMode }) {
  if (mode === "region") {
    return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 9V6a1 1 0 0 1 1-1h3M15 5h3a1 1 0 0 1 1 1v3M19 15v3a1 1 0 0 1-1 1h-3M9 19H6a1 1 0 0 1-1-1v-3" /><rect x="9" y="9" width="6" height="6" rx="1" /></svg>;
  }
  if (mode === "window") {
    return <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="4" y="6" width="16" height="13" rx="2.5" /><path d="M4 10h16M7 8h.01M10 8h.01" /></svg>;
  }
  return <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="4" width="18" height="14" rx="2.5" /><path d="M9 21h6M12 18v3" /></svg>;
}

/**
 * CSS/DIP size of the capture overlay for a display.
 *
 * On Windows, native display width/height are physical pixels while the overlay
 * and pointer events use logical DIPs. `window_coordinate_scale` is that DPI
 * factor (1 elsewhere).
 */
function displayOverlaySize(
  display: { width: number; height: number },
  windowCoordinateScale: number,
): { width: number; height: number } {
  const scale = Math.max(windowCoordinateScale || 1, 1);
  return {
    width: display.width / scale,
    height: display.height / scale,
  };
}

function roundRecordingRect(rect: RecordingRect, maxWidth: number, maxHeight: number): RecordingRect {
  const x = Math.max(0, Math.round(rect.x));
  const y = Math.max(0, Math.round(rect.y));
  return {
    x,
    y,
    width: Math.max(1, Math.min(maxWidth - x, Math.round(rect.width))),
    height: Math.max(1, Math.min(maxHeight - y, Math.round(rect.height))),
  };
}

type CaptureVisibilityContext = "screenshot" | "recording";

function captureOutputLabel(context: CaptureVisibilityContext): string {
  return context === "screenshot" ? "screenshots" : "recordings";
}

function recordingControlsVisibilityText(
  controlsExcluded: boolean | null,
  context: CaptureVisibilityContext,
  showHideHint = false,
): ReactNode {
  const output = captureOutputLabel(context);
  if (controlsExcluded === true) {
    return <>These controls <strong>won’t</strong> show in {output}</>;
  }
  if (controlsExcluded === false) {
    return showHideHint
      ? <>These controls <strong>will</strong> show in {output} · Use Hide controls to keep them out</>
      : <>These controls <strong>will</strong> show in {output}</>;
  }
  return "Checking whether these controls will show…";
}

function CaptureSelectorVisibilityNote({
  canExcludeControls,
  controlsExcluded,
  actionMode,
  onOpenPreference,
}: {
  canExcludeControls: boolean;
  controlsExcluded: boolean | null;
  actionMode: CaptureVisibilityContext;
  onOpenPreference: () => void;
}) {
  const copy = recordingControlsVisibilityText(
    controlsExcluded,
    actionMode,
    !canExcludeControls && actionMode === "recording",
  );
  if (!canExcludeControls) return copy;
  return (
    <CapturePreferenceLink onClick={onOpenPreference}>
      {copy}
    </CapturePreferenceLink>
  );
}

function CapturePreferenceLink({
  onClick,
  children,
}: {
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      className="capture-selector-preferences-link"
      type="button"
      onClick={onClick}
    >
      {children}
      <ExternalPreferenceIcon />
    </button>
  );
}

function ExternalPreferenceIcon() {
  return (
    <svg className="capture-selector-preferences-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path d="M6.5 3H4a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1V9.5" />
      <path d="M9 3h4v4M8.5 7.5 13 3" />
    </svg>
  );
}

export function RecordingHud() {
  const [snapshot, setSnapshot] = useState<RecordingSessionSnapshot | null>(null);
  const [controlsExcluded, setControlsExcluded] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [microphonePeak, setMicrophonePeak] = useState(0);
  const [error, setError] = useState("");
  const sessionIdRef = useRef<string | null>(null);

  useEffect(() => {
    let active = true;
    const dispose: (() => void)[] = [];
    const refreshControlsExcluded = () => {
      void invoke<boolean>("recording_controls_are_excluded")
        .then((excluded) => {
          if (active) setControlsExcluded(excluded);
        })
        .catch(() => {
          if (active) setControlsExcluded(false);
        });
    };
    const applySnapshot = (next: RecordingSessionSnapshot) => {
      if (!active) return;
      if (sessionIdRef.current !== next.id) {
        sessionIdRef.current = next.id;
        setMicrophonePeak(0);
        setError("");
      }
      setSnapshot(next);
    };
    void (async () => {
      const listeners = Promise.allSettled([
        listen<RecordingSessionSnapshot>("recording-state-changed", ({ payload }) => {
          applySnapshot(payload);
        }),
        listen<{ session_id: string; message: string }>("recording-warning", ({ payload }) => {
          if (active && payload.session_id === sessionIdRef.current) {
            setError(recordingErrorMessage(payload.message));
          }
        }),
        listen<{ session_id: string; microphone_peak: number }>("recording-audio-level", ({ payload }) => {
          if (active && payload.session_id === sessionIdRef.current) {
            setMicrophonePeak(Math.max(0, Math.min(1, payload.microphone_peak)));
          }
        }),
        // Update the privacy menu text as soon as the include preference changes.
        listen<AppSettings>("settings-changed", () => {
          refreshControlsExcluded();
        }),
      ]);
      void listeners.then((results) => {
        const unlisteners = results.flatMap((listener) => listener.status === "fulfilled" ? [listener.value] : []);
        if (active) {
          dispose.push(...unlisteners);
        } else {
          unlisteners.forEach((unlisten) => unlisten());
        }
      });
      const current = await invoke<RecordingSessionSnapshot | null>("get_recording_snapshot");
      if (current) applySnapshot(current);
    })();
    refreshControlsExcluded();
    const timer = window.setInterval(() => {
      void invoke<RecordingSessionSnapshot | null>("get_recording_snapshot").then((current) => {
        if (current) applySnapshot(current);
      });
    }, 250);
    return () => {
      active = false;
      window.clearInterval(timer);
      dispose.forEach((unlisten) => unlisten());
    };
  }, []);

  if (!snapshot) return <main className="recording-hud recording-hud-loading">Preparing…</main>;
  const invokeAction = async (command: string, extra: Record<string, unknown> = {}) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const result = await invoke<RecordingSessionSnapshot | RecordingArtifact>(command, {
        sessionId: snapshot.id,
        ...extra,
      });
      if ("state" in result) setSnapshot(result);
    } catch (error) {
      setError(recordingErrorMessage(error));
    } finally {
      setBusy(false);
    }
  };
  const takeScreenshot = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      // The native capture path temporarily content-protects this HUD while
      // taking its background snapshot, so it can stay visually stable here.
      await invoke("start_capture", { mode: "region" });
    } catch (error) {
      setError(recordingErrorMessage(error));
    } finally {
      setBusy(false);
    }
  };
  const hideControls = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await invoke("hide_recording_hud", {
        sessionId: snapshot.id,
      });
    } catch (error) {
      setError(recordingErrorMessage(error));
    } finally {
      setBusy(false);
    }
  };
  const startHudDrag = (event: React.PointerEvent<HTMLElement>) => {
    if (event.button !== 0 || !currentWindow) return;
    if ((event.target as Element).closest("button, input, a, [role='button'], [role='slider']")) return;
    event.preventDefault();
    void currentWindow.startDragging().catch((error) => setError(String(error)));
  };
  const canControl = snapshot.state === "recording" || snapshot.state === "paused";
  const canRestart = canControl || snapshot.state === "failed";
  const hasMicrophone = Boolean(snapshot.options.audio.microphone_device_id);
  const deleteRecording = async () => {
    if (busy) return;
    try {
      const choice = await message(
        "This recording will be deleted permanently.",
        {
          title: "Delete recording?",
          kind: "warning",
          buttons: { ok: "Delete", cancel: "Cancel" },
        },
      );
      if (choice === "Delete") {
        await invokeAction("discard_recording");
      }
    } catch (error) {
      setError(String(error));
    }
  };
  const restartRecording = async () => {
    if (busy) return;
    if (snapshot.state === "failed") {
      await invokeAction("restart_recording");
      return;
    }
    try {
      const choice = await message(
        "The current recording will be deleted and a new countdown will begin.",
        {
          title: "Restart recording?",
          kind: "warning",
          buttons: { ok: "Restart", cancel: "Cancel" },
        },
      );
      if (choice === "Restart") {
        await invokeAction("restart_recording");
      }
    } catch (error) {
      setError(String(error));
    }
  };

  return (
    <main
      className={`recording-hud on-media recording-hud-${snapshot.state}`}
      onPointerDown={startHudDrag}
    >
      <span className="recording-hud-privacy">
        {recordingControlsVisibilityText(controlsExcluded, "recording", true)}
      </span>
      <div className="recording-hud-main">
        <div className="recording-hud-status">
          <span className="recording-dot" aria-hidden="true" />
          <strong>{formatRecordingTime(snapshot.elapsed_ms)}</strong>
          <small>{recordingStatusLabel(snapshot)}</small>
        </div>
        <div className="recording-hud-actions">
          <HudTooltip label="Stop and save">
            <button type="button" className="recording-stop" disabled={!canControl || busy} aria-label="Stop recording" onClick={() => void invokeAction("stop_recording")}><span /></button>
          </HudTooltip>
          <HudTooltip label={snapshot.state === "paused" ? "Resume recording" : "Pause recording"}>
            <button
              type="button"
              className="recording-icon-button"
              disabled={!canControl || busy}
              aria-label={snapshot.state === "paused" ? "Resume recording" : "Pause recording"}
              onClick={() => void invokeAction(snapshot.state === "paused" ? "resume_recording" : "pause_recording")}
            ><PauseResumeIcon paused={snapshot.state === "paused"} /></button>
          </HudTooltip>
          <HudTooltip label={snapshot.state === "failed" ? "Retry recording" : "Restart recording"}>
            <button type="button" className="recording-icon-button" disabled={!canRestart || busy} aria-label={snapshot.state === "failed" ? "Retry recording" : "Restart recording"} onClick={() => void restartRecording()}><RestartRecordingIcon /></button>
          </HudTooltip>
          <HudTooltip label="Take a region screenshot">
            <button type="button" className="recording-icon-button" disabled={!canControl || busy} aria-label="Take a region screenshot" onClick={() => void takeScreenshot()}><CaptureIcon /></button>
          </HudTooltip>
          {hasMicrophone && (
            <span className="recording-microphone-level" aria-label={`Microphone level ${Math.round(microphonePeak * 100)}%`}>
              <i style={{ width: `${Math.round(microphonePeak * 100)}%` }} />
            </span>
          )}
          <HudTooltip label={snapshot.options.audio.microphone_muted ? "Unmute microphone" : "Mute microphone"}>
            <button
              type="button"
              disabled={!hasMicrophone || !canControl || busy}
              className={`recording-icon-button${snapshot.options.audio.microphone_muted ? " active" : ""}`}
              aria-label={snapshot.options.audio.microphone_muted ? "Unmute microphone" : "Mute microphone"}
              onClick={() => void invokeAction("set_recording_microphone_muted", { muted: !snapshot.options.audio.microphone_muted })}
            ><MicrophoneIcon muted={snapshot.options.audio.microphone_muted} /></button>
          </HudTooltip>
          <HudTooltip label="Delete recording">
            <button
              type="button"
              className="recording-icon-button recording-discard"
              disabled={busy || snapshot.state === "finalizing"}
              aria-label="Delete recording"
              onClick={() => void deleteRecording()}
            ><TrashIcon /></button>
          </HudTooltip>
          <HudTooltip label="Hide controls">
            <button
              type="button"
              className="recording-icon-button recording-hide"
              disabled={busy}
              aria-label="Hide recording controls"
              onClick={() => void hideControls()}
            ><HideControlsIcon /></button>
          </HudTooltip>
        </div>
      </div>
      {(error || snapshot.error) && (
        <p className="recording-hud-error" role="alert">
          {recordingErrorMessage(error || snapshot.error)}
        </p>
      )}
    </main>
  );
}

function recordingStatusLabel(snapshot: RecordingSessionSnapshot): string {
  if (snapshot.state === "countdown") return "Starting…";
  if (snapshot.state === "paused") return "Paused";
  if (snapshot.state === "finalizing") return "Saving…";
  if (snapshot.state === "failed") return "Failed";
  return "Recording";
}

function formatRecordingTime(milliseconds: number): string {
  const totalSeconds = Math.floor(milliseconds / 1_000);
  const hours = Math.floor(totalSeconds / 3_600);
  const minutes = Math.floor((totalSeconds % 3_600) / 60);
  const seconds = totalSeconds % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`
    : `${minutes}:${String(seconds).padStart(2, "0")}`;
}

function recordingErrorMessage(value: unknown): string {
  let message = String(value).trim();
  const prefix = /^(?:error|background task failed|recording failed):\s*/i;
  while (prefix.test(message)) message = message.replace(prefix, "");
  if (/^(?:the|a|an)\s/.test(message)) {
    message = message[0].toUpperCase() + message.slice(1);
  }
  return message || "The recording could not be completed.";
}

type EditorCropDrag = {
  handle: EditorCropHandle;
  start: SelectionPoint;
  initial: RecordingRect;
};

type FileSizeUnit = "kb" | "mb" | "gb";

/** Compress presets for the video editor (mirrors photo Tiny → Highest ladder). */
const RECORDING_QUALITY_OPTIONS = [
  {
    value: "tiny",
    label: "Tiny",
    description: "Smallest file with the most visible compression.",
  },
  {
    value: "small",
    label: "Smaller",
    description: "Very small file with more visible compression.",
  },
  {
    value: "standard",
    label: "Balanced",
    description: "Good quality with a meaningfully smaller file.",
  },
  {
    value: "high",
    label: "High",
    description: "Much smaller file with little visible quality loss.",
  },
  {
    value: "highest",
    label: "Highest",
    description: "Light compression. Near-original quality, a modest size cut.",
  },
] as const;

type RecordingCompressQuality = (typeof RECORDING_QUALITY_OPTIONS)[number]["value"];

/** Keep in sync with GIF palette floors in the export toolchain. */
function gifMaxColorsForQuality(quality: RecordingCompressQuality): number {
  switch (quality) {
    case "tiny":
      return 64;
    case "small":
      return 96;
    case "standard":
      return 128;
    case "high":
    case "highest":
      return 256;
  }
}

type RecordingEditorFingerprint = {
  artifact: string;
  makeCopy: boolean;
  filenameStem: string;
  destinationDirectory: string;
  trimStart: number;
  trimEnd: number;
  crop: RecordingRect | null;
  resolution: "original" | "1080" | "720" | "custom";
  customWidth: number;
  customHeight: number;
  outputFormat: "mp4" | "gif" | "webm";
  gifFps: number;
  gifMaxWidth: number;
  quality: RecordingCompressQuality;
  sizeMode: "preserve" | "compress" | "maximum";
  maximumSize: string;
  maximumUnit: FileSizeUnit;
  systemVolume: number;
  microphoneVolume: number;
  muteSystem: boolean;
  muteMicrophone: boolean;
  mono: boolean;
};

function recordingEditorFingerprint(value: RecordingEditorFingerprint): string {
  return JSON.stringify(value);
}

const FILE_SIZE_UNIT_BYTES: Record<FileSizeUnit, number> = {
  kb: 1_000,
  mb: 1_000_000,
  gb: 1_000_000_000,
};

export function RecordingEditor() {
  const artifactId = query("artifact_id");
  const [artifact, setArtifact] = useState<RecordingArtifact | null>(null);
  const [timeline, setTimeline] = useState<RecordingTimelinePreview | null>(null);
  const [playheadMs, setPlayheadMs] = useState(0);
  const [previewMode, setPreviewMode] = useState<"fit" | "actual">("fit");
  const [trimStart, setTrimStart] = useState(0);
  const [trimEnd, setTrimEnd] = useState(0);
  const [cropEnabled, setCropEnabled] = useState(false);
  const [crop, setCrop] = useState({ x: 0, y: 0, width: 1, height: 1 });
  const [aspectLocked, setAspectLocked] = useState(true);
  const [resolution, setResolution] = useState<"original" | "1080" | "720" | "custom">("original");
  const [customWidth, setCustomWidth] = useState(1920);
  const [customHeight, setCustomHeight] = useState(1080);
  const [outputFormat, setOutputFormat] = useState<"mp4" | "gif" | "webm">("mp4");
  const [gifFps, setGifFps] = useState(15);
  const [gifMaxWidth, setGifMaxWidth] = useState(800);
  const [quality, setQuality] = useState<RecordingCompressQuality>("highest");
  const [sizeMode, setSizeMode] = useState<"preserve" | "compress" | "maximum">("preserve");
  const [maximumSize, setMaximumSize] = useState("10");
  const [maximumUnit, setMaximumUnit] = useState<FileSizeUnit>("mb");
  const [systemVolume, setSystemVolume] = useState(100);
  const [microphoneVolume, setMicrophoneVolume] = useState(100);
  const [muteSystem, setMuteSystem] = useState(false);
  const [muteMicrophone, setMuteMicrophone] = useState(false);
  const [mono, setMono] = useState(false);
  const [exportId, setExportId] = useState<string | null>(null);
  const exportIdRef = useRef<string | null>(null);
  const [progress, setProgress] = useState<ExportProgress | null>(null);
  const [exported, setExported] = useState<RecordingArtifact | null>(null);
  const [savedFingerprint, setSavedFingerprint] = useState<string | null>(null);
  const [filenameStem, setFilenameStem] = useState("");
  const [destinationDirectory, setDestinationDirectory] = useState("");
  const [makeCopy, setMakeCopy] = useState(false);
  const [previewPlaying, setPreviewPlaying] = useState(false);
  const [previewLoop, setPreviewLoop] = useState(false);
  const [toast, setToast] = useState("");
  const [error, setError] = useState("");
  const [estimatedBytes, setEstimatedBytes] = useState<number | null>(null);
  const [estimateExact, setEstimateExact] = useState(false);
  const [estimatePending, setEstimatePending] = useState(false);
  const estimateRequestRef = useRef(0);
  const [compressPreviewPending, setCompressPreviewPending] = useState(false);
  const [compressPreviewError, setCompressPreviewError] = useState("");
  const [compressPreviewBeforeUrl, setCompressPreviewBeforeUrl] = useState<string | null>(null);
  const [compressPreviewAfterUrl, setCompressPreviewAfterUrl] = useState<string | null>(null);
  const [compressCompareDismissed, setCompressCompareDismissed] = useState(false);
  const [compressSplit, setCompressSplit] = useState(50);
  const compressPreviewUrlsRef = useRef<{ before: string | null; after: string | null }>({
    before: null,
    after: null,
  });
  // Monotonic id so an older in-flight preview encode cannot overwrite the
  // result of a newer one when responses arrive out of order.
  const compressPreviewRequestRef = useRef(0);
  const playheadMsRef = useRef(0);
  const videoRef = useRef<HTMLVideoElement>(null);
  const previewMediaRef = useRef<HTMLDivElement>(null);
  const timelineRef = useRef<HTMLDivElement>(null);
  const timelineScrubbingRef = useRef(false);
  const trimDragRef = useRef<"start" | "end" | null>(null);
  /** Pending trim-handle press; edge only moves after a small drag threshold. */
  const trimPointerRef = useRef<{
    edge: "start" | "end";
    startX: number;
    dragging: boolean;
  } | null>(null);
  const cropDragRef = useRef<EditorCropDrag | null>(null);
  const pendingExportFingerprintRef = useRef("");
  const trimStartRef = useRef(0);
  const trimEndRef = useRef(0);
  const previewLoopRef = useRef(false);
  const TRIM_DRAG_THRESHOLD_PX = 3;

  useEffect(() => {
    let active = true;
    const cleanup = createCleanupRegistry();
    void (async () => {
      const listeners = await Promise.all([
        listen<{ export_id: string; progress: ExportProgress }>("recording-export-progress", ({ payload }) => {
          if (active && payload.export_id === exportIdRef.current) setProgress(payload.progress);
        }),
        listen<{ export_id: string; artifact: RecordingArtifact; reveal_error: string | null }>("recording-export-complete", ({ payload }) => {
          if (!active || payload.export_id !== exportIdRef.current) return;
          if (payload.artifact.id === artifactId) {
            setArtifact(payload.artifact);
          }
          setExported(payload.artifact);
          setSavedFingerprint(pendingExportFingerprintRef.current);
          setToast(
            `${payload.artifact.kind === "gif" ? "GIF" : "Video"} saved — ${formatFileSize(payload.artifact.size_bytes)}.${
              payload.reveal_error ? " Its folder could not be opened." : ""
            }`,
          );
          setProgress({ stage: "complete", completed_per_mille: 1000, attempt: 1, message: null });
          setExportId(null);
          exportIdRef.current = null;
        }),
        listen<{ export_id: string; message: string; cancelled: boolean }>("recording-export-failed", ({ payload }) => {
          if (!active || payload.export_id !== exportIdRef.current) return;
          if (payload.cancelled) {
            setToast("Save cancelled.");
            setProgress({ stage: "cancelled", completed_per_mille: 0, attempt: 0, message: null });
          } else {
            setError(recordingErrorMessage(payload.message));
          }
          setExportId(null);
          exportIdRef.current = null;
        }),
      ]);
      if (!cleanup.add(...listeners)) return;
      if (!artifactId) return;
      const [loaded, loadedSettings] = await Promise.all([
        invoke<RecordingArtifact | null>("get_recording_artifact", { artifactId }),
        invoke<AppSettings>("get_settings").catch(() => null),
      ]);
      if (!active || !loaded) return;
      // Prefer a permanent Captures-folder save. Never default the footer to
      // private history recovery media (`media.mp4` under Capture History).
      const initialSave = recordingUserFacingDefaults({
        path: loaded.path,
        savedPath: loaded.saved_path,
        createdAt: loaded.created_at,
        outputDirectory: loadedSettings?.output_directory ?? "",
      });
      const initialFilenameStem = initialSave.stem;
      const initialDestinationDirectory = initialSave.directory;
      const preferredVideoFormat = loadedSettings?.recording.video_format ?? "mp4";
      const initialOutputFormat = loaded.kind === "gif"
        ? "gif"
        : preferredVideoFormat === "gif" || preferredVideoFormat === "webm"
          ? preferredVideoFormat
          : "mp4";
      const initialSizeMode = initialOutputFormat === "gif" ? "compress" : "preserve";
      const initialGifFps = loadedSettings?.recording.gif_fps ?? 15;
      const initialGifMaxWidth = loadedSettings?.recording.gif_max_width ?? 800;
      setArtifact(loaded);
      setTrimStart(0);
      setTrimEnd(loaded.duration_ms);
      setPlayheadMs(0);
      setCropEnabled(false);
      setCrop({ x: 0, y: 0, width: loaded.width, height: loaded.height });
      setResolution("original");
      setCustomWidth(loaded.width);
      setCustomHeight(loaded.height);
      setOutputFormat(initialOutputFormat);
      setGifFps(initialGifFps);
      setGifMaxWidth(initialGifMaxWidth);
      setQuality("highest");
      setSizeMode(initialSizeMode);
      setMaximumSize("10");
      setMaximumUnit("mb");
      setSystemVolume(100);
      setMicrophoneVolume(100);
      setMuteSystem(false);
      setMuteMicrophone(false);
      setMono(false);
      setFilenameStem(initialFilenameStem);
      setDestinationDirectory(initialDestinationDirectory);
      setMakeCopy(false);
      setPreviewPlaying(false);
      setExported(null);
      setSavedFingerprint(null);
      void invoke<RecordingTimelinePreview>("prepare_recording_timeline_preview", {
        artifactId: loaded.id,
      }).then((preview) => {
        if (active) setTimeline(preview);
      }).catch(() => {
        if (active) setTimeline(null);
      });
    })().catch((error) => {
      if (active) setError(recordingErrorMessage(error));
    });
    return () => {
      active = false;
      cleanup.dispose();
    };
  }, [artifactId]);

  // Recording editors are one artifact per window — mark the source as in-editor.
  useEffect(() => {
    if (!artifactId || !artifact) return;
    const editorId = `recording-editor-${artifactId}`;
    const publish = (artifactIds: string[]) => {
      void Promise.resolve(emit<EditorLayerPresence>("editor-layers-changed", {
        editor_id: editorId,
        artifact_ids: artifactIds,
      })).catch(() => undefined);
    };
    publish([artifactId]);
    return () => {
      publish([]);
    };
  }, [artifact, artifactId]);

  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(""), 4_000);
    return () => window.clearTimeout(timer);
  }, [toast]);

  useEffect(() => {
    trimStartRef.current = trimStart;
  }, [trimStart]);
  useEffect(() => {
    trimEndRef.current = trimEnd;
  }, [trimEnd]);
  useEffect(() => {
    previewLoopRef.current = previewLoop;
  }, [previewLoop]);

  // Drive the timeline playhead from the video clock every frame while playing so
  // it moves smoothly instead of jumping on sparse timeupdate events (~4 Hz).
  useEffect(() => {
    if (!previewPlaying) return;
    let frameId = 0;
    const tick = () => {
      const video = videoRef.current;
      if (video) {
        const currentMs = video.currentTime * 1_000;
        const selectedStart = trimStartRef.current;
        const selectedEnd = trimEndRef.current;
        if (!video.paused && currentMs >= selectedEnd) {
          if (previewLoopRef.current) {
            video.currentTime = selectedStart / 1_000;
            setPlayheadMs(selectedStart);
          } else {
            video.pause();
            video.currentTime = selectedEnd / 1_000;
            setPlayheadMs(selectedEnd);
            setPreviewPlaying(false);
          }
        } else if (!video.paused) {
          setPlayheadMs(currentMs);
        }
      }
      frameId = window.requestAnimationFrame(tick);
    };
    frameId = window.requestAnimationFrame(tick);
    return () => window.cancelAnimationFrame(frameId);
  }, [previewPlaying]);

  // Build the same edit/export payloads used by Save so background size
  // estimates and the before/after preview always match the real encode.
  const buildExportRequestSpecs = useCallback((): {
    edit: EditSpec;
    export: ExportSpec;
    maximumBytes: number | null;
  } | null => {
    if (!artifact) return null;
    const maximumBytes = sizeMode === "maximum"
      ? Math.floor(Number(maximumSize) * FILE_SIZE_UNIT_BYTES[maximumUnit])
      : null;
    const baseOutput = editorOutputDimensions(
      cropEnabled ? crop.width : artifact.width,
      cropEnabled ? crop.height : artifact.height,
      resolution,
      customWidth,
      customHeight,
    );
    const output = outputFormat === "gif"
      ? dimensionsAtMaximumWidth(baseOutput.width, baseOutput.height, gifMaxWidth)
      : baseOutput;
    const edit: EditSpec = {
      trim_start_ms: Math.round(trimStart),
      trim_end_ms: Math.round(trimEnd) >= artifact.duration_ms ? null : Math.round(trimEnd),
      crop: cropEnabled ? boundedCrop(crop, artifact.width, artifact.height) : null,
      output_width: resolution === "original" && outputFormat !== "gif" ? null : output.width,
      output_height: resolution === "original" && outputFormat !== "gif" ? null : output.height,
      audio: {
        system_volume: systemVolume / 100,
        microphone_volume: microphoneVolume / 100,
        mute_system_audio: outputFormat === "gif" || muteSystem,
        mute_microphone: outputFormat === "gif" || muteMicrophone,
        mono_output: mono,
        source_has_system_audio: artifact.has_system_audio,
        source_has_microphone_audio: artifact.has_microphone_audio,
      },
    };
    const exportSpec: ExportSpec = {
      format: outputFormat,
      quality: sizeMode === "preserve" ? "preserve" : sizeMode === "compress" ? quality : "preserve",
      max_size_bytes: maximumBytes,
      frames_per_second: outputFormat === "gif" ? gifFps : null,
      gif_max_colors: outputFormat === "gif" ? gifMaxColorsForQuality(quality) : null,
    };
    return { edit, export: exportSpec, maximumBytes };
  }, [
    artifact,
    crop,
    cropEnabled,
    customHeight,
    customWidth,
    gifFps,
    gifMaxWidth,
    maximumSize,
    maximumUnit,
    microphoneVolume,
    mono,
    muteMicrophone,
    muteSystem,
    outputFormat,
    quality,
    resolution,
    sizeMode,
    systemVolume,
    trimEnd,
    trimStart,
  ]);

  // Estimate the saved size in the background whenever settings change. The
  // backend encodes short samples with the save pipeline, so this takes a few
  // seconds and is debounced. Maximum mode shows the cap instead.
  useEffect(() => {
    if (!artifact || artifact.missing || exportId) return;
    // Maximum mode shows the cap instead of a sampled estimate; the label
    // ignores any stale estimate state, so nothing is cleared here.
    if (sizeMode === "maximum") return;
    if (outputFormat === "webm") return;
    const request = ++estimateRequestRef.current;
    const timer = window.setTimeout(() => {
      setEstimatePending(true);
      void (async () => {
        try {
          const specs = buildExportRequestSpecs();
          if (!specs) return;
          const estimate = await invoke<{ sizeBytes: number; exact: boolean }>(
            "estimate_recording_export",
            { artifactId: artifact.id, edit: specs.edit, export: specs.export },
          );
          if (estimateRequestRef.current !== request) return;
          setEstimatedBytes(estimate.sizeBytes);
          setEstimateExact(estimate.exact);
        } catch {
          if (estimateRequestRef.current === request) setEstimatedBytes(null);
        } finally {
          if (estimateRequestRef.current === request) setEstimatePending(false);
        }
      })();
    }, 600);
    return () => window.clearTimeout(timer);
  }, [artifact, buildExportRequestSpecs, exportId, outputFormat, sizeMode]);

  useEffect(() => {
    playheadMsRef.current = playheadMs;
  }, [playheadMs]);

  const revokeCompressPreviewUrls = useCallback(() => {
    const { before, after } = compressPreviewUrlsRef.current;
    if (before) URL.revokeObjectURL(before);
    if (after) URL.revokeObjectURL(after);
    compressPreviewUrlsRef.current = { before: null, after: null };
    setCompressPreviewBeforeUrl(null);
    setCompressPreviewAfterUrl(null);
  }, []);

  // Revoke any cached preview object URLs when the editor unmounts.
  useEffect(() => () => {
    const { before, after } = compressPreviewUrlsRef.current;
    if (before) URL.revokeObjectURL(before);
    if (after) URL.revokeObjectURL(after);
  }, []);

  const loadCompressPreview = useCallback(async () => {
    if (!artifact) return;
    const request = ++compressPreviewRequestRef.current;
    setCompressPreviewPending(true);
    setCompressPreviewError("");
    // Local until ownership transfers to compressPreviewUrlsRef; anything
    // still local by `finally` (stale response or error) gets revoked.
    let beforeUrl: string | null = null;
    let afterUrl: string | null = null;
    try {
      const specs = buildExportRequestSpecs();
      if (!specs) return;
      const preview = await invoke<{ beforePng: number[]; afterPng: number[] }>(
        "preview_recording_export",
        {
          artifactId: artifact.id,
          edit: specs.edit,
          export: specs.export,
          atMs: Math.round(playheadMsRef.current),
        },
      );
      beforeUrl = URL.createObjectURL(
        new Blob([new Uint8Array(preview.beforePng)], { type: "image/png" }),
      );
      afterUrl = URL.createObjectURL(
        new Blob([new Uint8Array(preview.afterPng)], { type: "image/png" }),
      );
      if (compressPreviewRequestRef.current !== request) return;
      revokeCompressPreviewUrls();
      compressPreviewUrlsRef.current = { before: beforeUrl, after: afterUrl };
      setCompressPreviewBeforeUrl(beforeUrl);
      setCompressPreviewAfterUrl(afterUrl);
      beforeUrl = null;
      afterUrl = null;
    } catch (reason) {
      if (compressPreviewRequestRef.current === request) {
        setCompressPreviewError(recordingErrorMessage(reason));
      }
    } finally {
      if (beforeUrl) URL.revokeObjectURL(beforeUrl);
      if (afterUrl) URL.revokeObjectURL(afterUrl);
      if (compressPreviewRequestRef.current === request) {
        setCompressPreviewPending(false);
      }
    }
  }, [artifact, buildExportRequestSpecs, revokeCompressPreviewUrls]);

  const canPreviewCompression = (sizeMode === "compress" || sizeMode === "maximum")
    && outputFormat !== "webm";
  const showCompressCompare = canPreviewCompression
    && !previewPlaying
    && !compressCompareDismissed;

  const clearCompressPreview = useCallback(() => {
    compressPreviewRequestRef.current += 1;
    revokeCompressPreviewUrls();
    setCompressPreviewPending(false);
    setCompressPreviewError("");
  }, [revokeCompressPreviewUrls]);

  // Encode a fresh sample whenever compress/maximum is active. Skip while the
  // preview is playing so we don't re-encode every frame; compare hides then too.
  useEffect(() => {
    if (!canPreviewCompression || previewPlaying) return;
    const timer = window.setTimeout(() => {
      void loadCompressPreview();
    }, 350);
    return () => window.clearTimeout(timer);
  }, [canPreviewCompression, loadCompressPreview, playheadMs, previewPlaying]);

  const exportFingerprint = artifact ? recordingEditorFingerprint({
    artifact: artifact.id,
    makeCopy,
    filenameStem,
    destinationDirectory,
    trimStart: Math.round(trimStart),
    trimEnd: Math.round(trimEnd),
    crop: cropEnabled ? boundedCrop(crop, artifact.width, artifact.height) : null,
    resolution,
    customWidth,
    customHeight,
    outputFormat,
    gifFps,
    gifMaxWidth,
    quality,
    sizeMode,
    maximumSize,
    maximumUnit,
    systemVolume,
    microphoneVolume,
    muteSystem,
    muteMicrophone,
    mono,
  }) : null;

  if (!artifact || !exportFingerprint) {
    return <main className="recording-editor recording-editor-loading">{error || "Loading recording…"}</main>;
  }

  const duration = Math.max(1, artifact.duration_ms);
  const trimmedDuration = Math.max(1, trimEnd - trimStart);
  const baseOutputDimensions = editorOutputDimensions(
    cropEnabled ? crop.width : artifact.width,
    cropEnabled ? crop.height : artifact.height,
    resolution,
    customWidth,
    customHeight,
  );
  const hasRecordedAudio = artifact.has_system_audio || artifact.has_microphone_audio;
  // User-facing original is the permanent Captures save when present — not the
  // private history recovery path (`…/history/<id>/media.mp4`).
  const originalSave = recordingUserFacingDefaults({
    path: artifact.path,
    savedPath: artifact.saved_path,
    createdAt: artifact.created_at,
    outputDirectory: destinationDirectory,
  });
  const sourceDirectory = originalSave.directory;
  const sourceStem = originalSave.stem;
  const sourceFormat = artifact.kind === "gif" ? "gif" : "mp4";
  const formatRequiresCopy = outputFormat !== sourceFormat;
  const alreadySaved = Boolean(exported && savedFingerprint === exportFingerprint);
  const maximumBytes = sizeMode === "maximum"
    ? Math.floor(Number(maximumSize) * FILE_SIZE_UNIT_BYTES[maximumUnit])
    : null;
  const maximumBytesValid = maximumBytes !== null
    && Number.isFinite(maximumBytes)
    && maximumBytes >= 100_000;
  const estimatedSizeLabel = sizeMode === "maximum"
    ? maximumBytesValid && maximumBytes !== null ? `≤ ${formatFileSize(maximumBytes)}` : "—"
    : outputFormat === "webm"
      ? "—"
      : estimatePending && estimatedBytes === null
        ? "Estimating…"
        : estimatedBytes === null
          ? "—"
          : `${estimateExact ? "" : "≈ "}${formatFileSize(estimatedBytes)}`;
  const estimatedDelta = sizeMode === "maximum" || outputFormat === "webm" || estimatePending
    ? null
    : formatFileSizeDelta(estimatedBytes, artifact.size_bytes);
  const saveStatus = error
    || toast
    || (exportId ? progress?.message || exportStageLabel(progress?.stage || "preparing") : "");
  const updateMakeCopy = (enabled: boolean) => {
    if (!enabled && formatRequiresCopy) return;
    setMakeCopy(enabled);
    if (enabled && filenameStem === sourceStem && destinationDirectory === sourceDirectory) {
      setFilenameStem(recordingEditedFileStem(sourceStem));
    } else if (!enabled && filenameStem === recordingEditedFileStem(sourceStem)) {
      setFilenameStem(sourceStem);
    }
    setSavedFingerprint(null);
    setExported(null);
    setToast("");
    setError("");
  };
  const updateOutputFormat = (format: "mp4" | "gif" | "webm") => {
    setOutputFormat(format);
    if (format !== sourceFormat && !makeCopy) {
      setMakeCopy(true);
      setFilenameStem(recordingEditedFileStem(sourceStem));
      setDestinationDirectory(sourceDirectory);
    }
    if (format === "gif" && sizeMode === "preserve") setSizeMode("compress");
  };

  const updateCropDimension = (key: "width" | "height", value: number) => {
    setCrop((current) => {
      const maximumWidth = Math.max(2, artifact.width - current.x);
      const maximumHeight = Math.max(2, artifact.height - current.y);
      const ratio = current.width / Math.max(1, current.height);
      if (!aspectLocked) {
        return key === "width"
          ? { ...current, width: clampNumber(Math.round(value), 2, maximumWidth) }
          : { ...current, height: clampNumber(Math.round(value), 2, maximumHeight) };
      }
      if (key === "width") {
        let width = clampNumber(Math.round(value), 2, maximumWidth);
        let height = Math.max(2, Math.round(width / ratio));
        if (height > maximumHeight) {
          height = maximumHeight;
          width = Math.max(2, Math.round(height * ratio));
        }
        return { ...current, width, height };
      }
      let height = clampNumber(Math.round(value), 2, maximumHeight);
      let width = Math.max(2, Math.round(height * ratio));
      if (width > maximumWidth) {
        width = maximumWidth;
        height = Math.max(2, Math.round(width / ratio));
      }
      return { ...current, width, height };
    });
  };

  const updateCropOrigin = (key: "x" | "y", value: number) => {
    setCrop((current) => {
      if (key === "x") {
        const x = clampNumber(
          Math.round(value),
          0,
          Math.max(0, artifact.width - current.width),
        );
        return { ...current, x };
      }
      const y = clampNumber(
        Math.round(value),
        0,
        Math.max(0, artifact.height - current.height),
      );
      return { ...current, y };
    });
  };

  const seekTo = (milliseconds: number) => {
    const next = clampNumber(milliseconds, 0, duration);
    setPlayheadMs(next);
    if (videoRef.current) videoRef.current.currentTime = next / 1_000;
  };
  const timelineTimeAtPointer = (clientX: number) => {
    const bounds = timelineRef.current?.getBoundingClientRect();
    if (!bounds) return 0;
    return clampNumber(((clientX - bounds.left) / Math.max(1, bounds.width)) * duration, 0, duration);
  };
  const updateTimelinePointer = (clientX: number) => {
    const next = timelineTimeAtPointer(clientX);
    if (trimDragRef.current === "start") {
      const value = Math.min(next, trimEnd - 1);
      setTrimStart(value);
      seekTo(value);
    } else if (trimDragRef.current === "end") {
      const value = Math.max(next, trimStart + 1);
      setTrimEnd(value);
      seekTo(value);
    } else if (timelineScrubbingRef.current) {
      seekTo(next);
    }
  };
  const beginTrimHandlePointer = (
    edge: "start" | "end",
    event: React.PointerEvent<HTMLButtonElement>,
  ) => {
    trimPointerRef.current = {
      edge,
      startX: event.clientX,
      dragging: false,
    };
    trimDragRef.current = null;
    seekTo(edge === "start" ? trimStart : trimEnd);
    event.currentTarget.setPointerCapture(event.pointerId);
    event.preventDefault();
    event.stopPropagation();
  };
  const moveTrimHandlePointer = (event: React.PointerEvent<HTMLButtonElement>) => {
    const pending = trimPointerRef.current;
    if (!pending) return;
    if (!pending.dragging) {
      if (Math.abs(event.clientX - pending.startX) < TRIM_DRAG_THRESHOLD_PX) return;
      pending.dragging = true;
      trimDragRef.current = pending.edge;
    }
    if (trimDragRef.current) updateTimelinePointer(event.clientX);
  };
  const endTrimHandlePointer = () => {
    trimPointerRef.current = null;
    trimDragRef.current = null;
  };
  const startCropDrag = (event: React.PointerEvent<HTMLElement>, handle: EditorCropHandle) => {
    if (!cropEnabled || !previewMediaRef.current) return;
    const bounds = previewMediaRef.current.getBoundingClientRect();
    cropDragRef.current = {
      handle,
      start: {
        x: ((event.clientX - bounds.left) / Math.max(1, bounds.width)) * artifact.width,
        y: ((event.clientY - bounds.top) / Math.max(1, bounds.height)) * artifact.height,
      },
      initial: crop,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
    event.preventDefault();
    event.stopPropagation();
  };
  const updateCropFromPointer = (event: React.PointerEvent<HTMLDivElement>) => {
    const drag = cropDragRef.current;
    const bounds = previewMediaRef.current?.getBoundingClientRect();
    if (!drag || !bounds) return;
    const current = {
      x: ((event.clientX - bounds.left) / Math.max(1, bounds.width)) * artifact.width,
      y: ((event.clientY - bounds.top) / Math.max(1, bounds.height)) * artifact.height,
    };
    setCrop(editorCropAfterDrag(
      drag.initial,
      drag.handle,
      { x: current.x - drag.start.x, y: current.y - drag.start.y },
      { width: artifact.width, height: artifact.height },
      aspectLocked,
    ));
  };

  const togglePreviewPlayback = async () => {
    const video = videoRef.current;
    if (!video) return;
    setError("");
    try {
      if (video.paused) {
        const selectedStart = trimStart / 1_000;
        const selectedEnd = trimEnd / 1_000;
        if (
          video.ended
          || video.currentTime < selectedStart - 0.01
          || video.currentTime >= selectedEnd - 0.01
        ) {
          video.currentTime = selectedStart;
          setPlayheadMs(trimStart);
        }
        await video.play();
      } else {
        video.pause();
      }
    } catch (error) {
      setPreviewPlaying(false);
      setError(`Preview could not play: ${String(error)}`);
    }
  };
  const updatePreviewPlaybackTime = (video: HTMLVideoElement) => {
    const currentMs = video.currentTime * 1_000;
    if (!video.paused && currentMs >= trimEnd) {
      if (previewLoop) {
        video.currentTime = trimStart / 1_000;
        setPlayheadMs(trimStart);
      } else {
        video.pause();
        video.currentTime = trimEnd / 1_000;
        setPlayheadMs(trimEnd);
        setPreviewPlaying(false);
      }
      return;
    }
    // While playing, requestAnimationFrame owns the playhead for smooth motion.
    if (video.paused) setPlayheadMs(currentMs);
  };
  const handlePreviewEnded = (video: HTMLVideoElement) => {
    if (!previewLoop) {
      setPreviewPlaying(false);
      return;
    }
    video.currentTime = trimStart / 1_000;
    setPlayheadMs(trimStart);
    void video.play().catch((error) => {
      setPreviewPlaying(false);
      setError(`Preview could not loop: ${String(error)}`);
    });
  };

  const chooseDestinationDirectory = async () => {
    if (exportId) return;
    setError("");
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Choose save location",
        defaultPath: destinationDirectory || sourceDirectory,
      });
      if (typeof selected === "string") {
        setDestinationDirectory(selected);
        setToast("");
      }
    } catch (error) {
      setError(`Save location could not be changed: ${String(error)}`);
    }
  };

  const revealSavedRecording = async () => {
    if (!exported) return;
    setError("");
    try {
      await invoke("reveal_recording_artifact", { artifactId: exported.id });
    } catch (error) {
      setError(`Could not show the recording in its folder: ${recordingErrorMessage(error)}`);
    }
  };

  const startExport = async () => {
    if (exportId) return;
    const invalidFilename = recordingFilenameError(filenameStem);
    if (invalidFilename) {
      setError(invalidFilename);
      return;
    }
    const specs = buildExportRequestSpecs();
    if (!specs) return;
    const { edit, export: exportSpec, maximumBytes } = specs;
    if (
      sizeMode === "maximum"
      && (maximumBytes === null || !Number.isFinite(maximumBytes) || maximumBytes < 100_000)
    ) {
      setError("Enter a maximum file size of at least 100 KB.");
      return;
    }
    setError("");
    setToast("");
    setExported(null);
    setProgress({ stage: "preparing", completed_per_mille: 0, attempt: 0, message: null });
    pendingExportFingerprintRef.current = exportFingerprint;
    try {
      const id = await invoke<string>("start_recording_export", {
        request: {
          artifact_id: artifact.id,
          file_stem: filenameStem,
          destination_directory: destinationDirectory,
          overwrite_source: !makeCopy && !formatRequiresCopy,
          edit,
          export: exportSpec,
        },
      });
      exportIdRef.current = id;
      setExportId(id);
    } catch (error) {
      setError(recordingErrorMessage(error));
      setProgress(null);
    }
  };

  return (
    <main className="recording-editor">
      <header className="recording-editor-header">
        <div><h1>{artifact.kind === "gif" ? "Edit GIF" : "Edit recording"}</h1></div>
      </header>
      {artifact.dropped_frames > 0 && <p className="recording-editor-warning" role="status">This source dropped {artifact.dropped_frames.toLocaleString()} frame{artifact.dropped_frames === 1 ? "" : "s"} during capture. The original timing is preserved.</p>}

      <section className="recording-editor-preview">
        <div className="recording-preview-toolbar">
          <strong>Preview</strong>
          <div className="recording-preview-toolbar-actions">
            {artifact.kind === "video" && (
              <button
                type="button"
                className={`recording-preview-loop${previewLoop ? " active" : ""}`}
                aria-pressed={previewLoop}
                onClick={() => setPreviewLoop((current) => !current)}
              ><span aria-hidden="true">↻</span>Loop preview</button>
            )}
            <div
              className="editor-segmented preview-size-segmented"
              role="group"
              aria-label="Preview size"
              data-active={previewMode}
            >
              <SegmentedControlIndicator value={previewMode} />
              <button
                type="button"
                className={previewMode === "fit" ? "active" : ""}
                aria-pressed={previewMode === "fit"}
                onClick={() => setPreviewMode("fit")}
              >Fit</button>
              <button
                type="button"
                className={previewMode === "actual" ? "active" : ""}
                aria-pressed={previewMode === "actual"}
                onClick={() => setPreviewMode("actual")}
              >100%</button>
            </div>
          </div>
        </div>
        <div className={`recording-preview-viewport preview-${previewMode}`}>
          <div
            ref={previewMediaRef}
            className="recording-preview-media"
            style={previewMode === "actual"
              ? { width: artifact.width, height: artifact.height }
              : {
                  width: `min(100%, ${(52 * artifact.width / Math.max(1, artifact.height)).toFixed(2)}vh)`,
                  aspectRatio: `${artifact.width} / ${artifact.height}`,
                }}
            onPointerMove={updateCropFromPointer}
            onPointerUp={() => {
              cropDragRef.current = null;
            }}
            onPointerCancel={() => {
              cropDragRef.current = null;
            }}
          >
            {artifact.kind === "video" ? (
              <video
                ref={videoRef}
                src={artifact.media_url}
                playsInline
                preload="auto"
                onClick={() => void togglePreviewPlayback()}
                onPlay={() => setPreviewPlaying(true)}
                onPause={() => setPreviewPlaying(false)}
                onEnded={(event) => handlePreviewEnded(event.currentTarget)}
                onLoadedMetadata={(event) => {
                  event.currentTarget.currentTime = playheadMs / 1_000;
                }}
                onTimeUpdate={(event) => updatePreviewPlaybackTime(event.currentTarget)}
                onSeeked={(event) => setPlayheadMs(event.currentTarget.currentTime * 1_000)}
              />
            ) : (
              <img src={artifact.media_url} alt="Animated GIF preview" />
            )}
            {showCompressCompare && (
              <CompressionPreview
                className="is-embed is-cover"
                beforeUrl={compressPreviewBeforeUrl}
                afterUrl={compressPreviewAfterUrl}
                beforeBytes={artifact.size_bytes}
                afterBytes={sizeMode === "maximum"
                  ? maximumBytesValid ? maximumBytes : null
                  : estimatedBytes}
                pending={compressPreviewPending}
                error={compressPreviewError}
                initialSplit={compressSplit}
                onSplitChange={setCompressSplit}
                onDismiss={() => setCompressCompareDismissed(true)}
              />
            )}
            {artifact.kind === "video" && (
              <button
                type="button"
                className={`recording-preview-overlay-play${previewPlaying ? " playing" : ""}`}
                aria-label={previewPlaying ? "Pause preview" : "Play preview"}
                onClick={() => void togglePreviewPlayback()}
              >
                <span aria-hidden="true">{previewPlaying ? "Ⅱ" : "▶"}</span>
              </button>
            )}
            {cropEnabled && (
              <div className="editor-crop-layer" aria-label="Crop recording">
                <i className="crop-dim crop-dim-top" style={{ height: `${crop.y / artifact.height * 100}%` }} />
                <i className="crop-dim crop-dim-left" style={{ top: `${crop.y / artifact.height * 100}%`, width: `${crop.x / artifact.width * 100}%`, height: `${crop.height / artifact.height * 100}%` }} />
                <i className="crop-dim crop-dim-right" style={{ top: `${crop.y / artifact.height * 100}%`, left: `${(crop.x + crop.width) / artifact.width * 100}%`, height: `${crop.height / artifact.height * 100}%` }} />
                <i className="crop-dim crop-dim-bottom" style={{ top: `${(crop.y + crop.height) / artifact.height * 100}%` }} />
                <div
                  className="editor-crop-box"
                  style={{
                    left: `${crop.x / artifact.width * 100}%`,
                    top: `${crop.y / artifact.height * 100}%`,
                    width: `${crop.width / artifact.width * 100}%`,
                    height: `${crop.height / artifact.height * 100}%`,
                  }}
                  onPointerDown={(event) => startCropDrag(event, "move")}
                >
                  <span>{Math.round(crop.width)} × {Math.round(crop.height)}</span>
                  {(["n", "ne", "e", "se", "s", "sw", "w", "nw"] as const).map((handle) => (
                    <button
                      key={handle}
                      type="button"
                      className={`crop-handle crop-handle-${handle}`}
                      aria-label={`Resize crop ${handle}`}
                      onPointerDown={(event) => startCropDrag(event, handle)}
                    />
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>
      </section>

      <section className="recording-timeline">
        <div className="timeline-summary"><strong>{formatEditorTime(trimStart, duration)} – {formatEditorTime(trimEnd, duration)}</strong><span>{formatEditorTime(trimmedDuration, duration)} selected</span></div>
        <div
          ref={timelineRef}
          className="timeline-track"
          aria-label="Recording timeline"
          onPointerDown={(event) => {
            if ((event.target as Element).closest(".timeline-trim-handle")) return;
            event.currentTarget.setPointerCapture(event.pointerId);
            timelineScrubbingRef.current = true;
            updateTimelinePointer(event.clientX);
          }}
          onPointerMove={(event) => {
            if (timelineScrubbingRef.current || trimDragRef.current) updateTimelinePointer(event.clientX);
          }}
          onPointerUp={() => {
            timelineScrubbingRef.current = false;
            endTrimHandlePointer();
          }}
          onPointerCancel={() => {
            timelineScrubbingRef.current = false;
            endTrimHandlePointer();
          }}
        >
          <div className="timeline-filmstrip" aria-hidden="true">
            {Array.from({ length: timeline?.frame_count ?? 12 }, (_, index) => (
              <i
                key={index}
                style={timeline ? {
                  backgroundImage: `url("${timeline.url}")`,
                  backgroundSize: `${timeline.frame_count * 100}% 100%`,
                  backgroundPosition: `${timeline.frame_count <= 1 ? 0 : index / (timeline.frame_count - 1) * 100}% 0`,
                } : undefined}
              />
            ))}
          </div>
          <div className="timeline-excluded timeline-excluded-start" style={{ width: `${trimStart / duration * 100}%` }} />
          <div className="timeline-excluded timeline-excluded-end" style={{ left: `${trimEnd / duration * 100}%` }} />
          <button
            type="button"
            role="slider"
            className="timeline-trim-handle timeline-trim-start"
            style={{ left: `${trimStart / duration * 100}%` }}
            aria-label="Trim start"
            aria-valuemin={0}
            aria-valuemax={Math.max(0, trimEnd - 1)}
            aria-valuenow={Math.round(trimStart)}
            aria-valuetext={formatEditorTime(trimStart, duration)}
            onPointerDown={(event) => beginTrimHandlePointer("start", event)}
            onPointerMove={moveTrimHandlePointer}
            onPointerUp={endTrimHandlePointer}
            onPointerCancel={endTrimHandlePointer}
            onKeyDown={(event) => {
              const delta = timelineKeyboardDelta(event.key, duration);
              if (delta === null) return;
              event.preventDefault();
              const next = clampNumber(trimStart + delta, 0, trimEnd - 1);
              setTrimStart(next);
              seekTo(next);
            }}
          ><span>{formatEditorTime(trimStart, duration)}</span></button>
          <button
            type="button"
            role="slider"
            className="timeline-trim-handle timeline-trim-end"
            style={{ left: `${trimEnd / duration * 100}%` }}
            aria-label="Trim end"
            aria-valuemin={Math.min(duration, trimStart + 1)}
            aria-valuemax={duration}
            aria-valuenow={Math.round(trimEnd)}
            aria-valuetext={formatEditorTime(trimEnd, duration)}
            onPointerDown={(event) => beginTrimHandlePointer("end", event)}
            onPointerMove={moveTrimHandlePointer}
            onPointerUp={endTrimHandlePointer}
            onPointerCancel={endTrimHandlePointer}
            onKeyDown={(event) => {
              const delta = timelineKeyboardDelta(event.key, duration);
              if (delta === null) return;
              event.preventDefault();
              const next = clampNumber(trimEnd + delta, trimStart + 1, duration);
              setTrimEnd(next);
              seekTo(next);
            }}
          ><span>{formatEditorTime(trimEnd, duration)}</span></button>
          <div className="timeline-playhead" style={{ left: `${playheadMs / duration * 100}%` }}><i /></div>
        </div>
      </section>

      <div className="recording-editor-grid">
        {outputFormat === "gif" && (
          <section className="editor-card editor-output-card">
            <h2>GIF settings</h2>
            <div className="editor-number-grid dimensions">
              <div className="editor-field"><span>Frame rate</span>
                <CustomSelect
                  value={String(gifFps)}
                  ariaLabel="GIF frame rate"
                  options={[8, 10, 12, 15, 20, 24, 30].map((value) => ({ value: String(value), label: `${value} FPS` }))}
                  onChange={(value) => setGifFps(Number(value))}
                />
              </div>
              <div className="editor-field"><span>Maximum width</span>
                <CustomSelect
                  value={String(gifMaxWidth)}
                  ariaLabel="GIF maximum width"
                  options={[320, 480, 640, 800, 1200].map((value) => ({ value: String(value), label: `${value} px` }))}
                  onChange={(value) => setGifMaxWidth(Number(value))}
                />
              </div>
            </div>
          </section>
        )}

        <section className="editor-card">
          <h2>Crop & size</h2>
          <label className="check-row"><input type="checkbox" checked={cropEnabled} onChange={(event) => setCropEnabled(event.target.checked)} /><span>Crop recording</span></label>
          <div className="editor-number-grid">
            <label>X
              <NumberInput
                min={0}
                max={Math.max(0, artifact.width - crop.width)}
                value={crop.x}
                disabled={!cropEnabled}
                onChange={(value) => updateCropOrigin("x", value)}
              />
            </label>
            <label>Y
              <NumberInput
                min={0}
                max={Math.max(0, artifact.height - crop.height)}
                value={crop.y}
                disabled={!cropEnabled}
                onChange={(value) => updateCropOrigin("y", value)}
              />
            </label>
            <label>Width
              <NumberInput
                min={2}
                max={Math.max(2, artifact.width - crop.x)}
                value={crop.width}
                disabled={!cropEnabled}
                onChange={(value) => updateCropDimension("width", value)}
              />
            </label>
            <label>Height
              <NumberInput
                min={2}
                max={Math.max(2, artifact.height - crop.y)}
                value={crop.height}
                disabled={!cropEnabled}
                onChange={(value) => updateCropDimension("height", value)}
              />
            </label>
          </div>
          <label className="check-row compact editor-aspect-lock"><input type="checkbox" checked={aspectLocked} onChange={(event) => setAspectLocked(event.target.checked)} /><span>Lock aspect ratio</span></label>
          <div className="editor-field editor-resolution-field"><span>Output resolution</span>
            <CustomSelect
              value={resolution}
              ariaLabel="Output resolution"
              options={[
                {
                  value: "original",
                  label: `Original — ${baseOutputDimensions.width} × ${baseOutputDimensions.height}`,
                  description: "Keep the recording’s pixel dimensions.",
                },
                {
                  value: "1080",
                  label: "1080p maximum",
                  description: "Scale down so the video is at most 1080 pixels tall.",
                },
                {
                  value: "720",
                  label: "720p maximum",
                  description: "Scale down so the video is at most 720 pixels tall.",
                },
                {
                  value: "custom",
                  label: "Custom",
                  description: "Choose exact pixel dimensions.",
                },
              ]}
              onChange={(value) => setResolution(value as typeof resolution)}
            />
          </div>
          {resolution === "custom" && (
            <div className="editor-number-grid dimensions">
              <label>Width
                <NumberInput min={2} value={customWidth} onChange={setCustomWidth} />
              </label>
              <label>Height
                <NumberInput min={2} value={customHeight} onChange={setCustomHeight} />
              </label>
            </div>
          )}
        </section>

        <section className="editor-card editor-quality-card">
          <h2>Save quality</h2>
          <div className="editor-field editor-quality-mode-field"><span>Quality mode</span>
            <CustomSelect
              value={sizeMode}
              ariaLabel="Save quality"
              options={[
                ...(outputFormat === "mp4" ? [{
                  value: "preserve",
                  label: "Preserve quality",
                  description: "Original quality with no extra compression unless an edit requires it.",
                }] : []),
                {
                  value: "compress",
                  label: "Compress",
                  description: "Choose a smaller file with Tiny through Highest quality presets.",
                },
                {
                  value: "maximum",
                  label: "Maximum file size",
                  description: "Set a hard size limit for the saved file.",
                },
              ]}
              onChange={(value) => {
                const mode = value as typeof sizeMode;
                setSizeMode(mode);
                if (mode === "preserve") {
                  clearCompressPreview();
                  setCompressCompareDismissed(false);
                  setCompressSplit(50);
                } else {
                  setCompressCompareDismissed(false);
                }
              }}
            />
          </div>
          <p className="editor-field-help">
            {sizeMode === "preserve"
              ? "Original quality with no extra compression unless an edit requires it."
              : sizeMode === "compress"
                ? "Choose a smaller file with Tiny through Highest quality presets."
                : "Set a hard size limit for the saved file."}
          </p>
          {sizeMode === "compress" && (
            <div className="editor-field editor-quality-preset">
              <span>Quality</span>
              <CustomSelect
                value={quality}
                ariaLabel="Compression quality"
                options={RECORDING_QUALITY_OPTIONS.map((option) => ({
                  value: option.value,
                  label: option.label,
                  description: option.description,
                }))}
                onChange={(value) => setQuality(value as RecordingCompressQuality)}
              />
            </div>
          )}
          {canPreviewCompression && compressCompareDismissed && (
            <div className="editor-field">
              <span>Comparison</span>
              <button
                type="button"
                onClick={() => setCompressCompareDismissed(false)}
              >
                Show before / after
              </button>
            </div>
          )}
          {sizeMode === "maximum" && (
            <div className="editor-field"><span>Maximum file size</span>
              <div className="editor-size-limit">
                <NumberInput
                  min={maximumUnit === "kb" ? 100 : maximumUnit === "mb" ? 0.1 : 0.0001}
                  step={maximumUnit === "kb" ? 1 : maximumUnit === "mb" ? 0.1 : 0.0001}
                  value={maximumSize}
                  ariaLabel="Maximum file size"
                  onTextChange={setMaximumSize}
                />
                <CustomSelect
                  value={maximumUnit}
                  ariaLabel="File size unit"
                  options={[
                    { value: "kb", label: "KB" },
                    { value: "mb", label: "MB" },
                    { value: "gb", label: "GB" },
                  ]}
                  onChange={(value) => {
                    const nextUnit = value as FileSizeUnit;
                    const bytes = Number(maximumSize) * FILE_SIZE_UNIT_BYTES[maximumUnit];
                    setMaximumUnit(nextUnit);
                    if (Number.isFinite(bytes)) {
                      setMaximumSize(formatMaximumFileSizeInput(bytes, nextUnit));
                    }
                  }}
                />
              </div>
            </div>
          )}
          <div className="editor-field recording-output-estimate-field" aria-live="polite">
            <span>Est. size</span>
            <strong
              className="recording-output-estimate"
              data-pending={sizeMode !== "maximum" && outputFormat !== "webm" && estimatePending ? "true" : undefined}
              title="Estimated saved file size for the current edits and settings"
            >
              {estimatedSizeLabel}
              {estimatedDelta && (
                <span
                  className={`recording-output-estimate-delta${estimatedDelta.percent < 0 ? " is-smaller" : " is-larger"}`}
                  title="Change versus the original recording file"
                >
                  {estimatedDelta.label}
                </span>
              )}
            </strong>
          </div>
        </section>

        {artifact.kind === "video" && outputFormat === "mp4" && hasRecordedAudio && <section className="editor-card editor-audio-card">
          <h2>Audio</h2>
          {artifact.has_system_audio && (
            <div className="editor-volume">
              <label>
                <input
                  type="checkbox"
                  checked={!muteSystem}
                  onChange={(event) => setMuteSystem(!event.target.checked)}
                />
                System audio
              </label>
              <RangeSlider
                ariaLabel="System audio volume"
                min={0}
                max={200}
                value={systemVolume}
                valueText={`${systemVolume}%`}
                disabled={muteSystem}
                onChange={setSystemVolume}
              />
            </div>
          )}
          {artifact.has_microphone_audio && (
            <div className="editor-volume">
              <label>
                <input
                  type="checkbox"
                  checked={!muteMicrophone}
                  onChange={(event) => setMuteMicrophone(!event.target.checked)}
                />
                Microphone
              </label>
              <RangeSlider
                ariaLabel="Microphone volume"
                min={0}
                max={200}
                value={microphoneVolume}
                valueText={`${microphoneVolume}%`}
                disabled={muteMicrophone}
                onChange={setMicrophoneVolume}
              />
            </div>
          )}
          <label className="check-row compact"><input type="checkbox" checked={mono} onChange={(event) => setMono(event.target.checked)} /><span>Convert to mono</span></label>
        </section>}
        {artifact.kind === "video" && outputFormat === "gif" && hasRecordedAudio && <section className="editor-card editor-audio-warning" role="status">
          <h2>Audio</h2>
          <p>GIFs do not include recorded audio.</p>
        </section>}
      </div>

      <footer className={`recording-save-footer${error ? " has-error" : ""}`}>
        {progress && exportId && <div className="recording-export-progress"><span style={{ width: `${progress.completed_per_mille / 10}%` }} /></div>}
        <div className="recording-filename">
          <div className="recording-filename-heading">
            <label htmlFor="recording-save-filename">Filename</label>
            <div className="recording-destination">
              <span>Saving to</span>
              <output aria-label="Save location" title={destinationDirectory}>{destinationDirectory}</output>
              <button
                type="button"
                aria-label="Change save location"
                disabled={Boolean(exportId)}
                onClick={() => void chooseDestinationDirectory()}
              >Change…</button>
            </div>
          </div>
          <span className="recording-filename-input">
            <input
              id="recording-save-filename"
              value={filenameStem}
              aria-label="Saved filename"
              spellCheck={false}
              disabled={Boolean(exportId)}
              onFocus={(event) => event.currentTarget.select()}
              onChange={(event) => {
                setFilenameStem(event.target.value);
                setError("");
              }}
            />
            <CustomSelect
              className="filename-format-select"
              value={outputFormat}
              ariaLabel="Format"
              triggerLabel={`.${outputFormat}`}
              disabled={Boolean(exportId)}
              options={[
                { value: "mp4", label: "MP4" },
                { value: "gif", label: "GIF" },
                { value: "webm", label: "WebM" },
              ]}
              onChange={(value) => updateOutputFormat(value as "mp4" | "gif" | "webm")}
            />
          </span>
        </div>
        <label
          className="recording-toggle recording-make-copy"
          title={formatRequiresCopy
            ? "Changing formats always creates a new file"
            : "Save as a new file and leave the original untouched"}
        >
          <input
            aria-label="Save as new file"
            type="checkbox"
            checked={makeCopy}
            disabled={Boolean(exportId) || formatRequiresCopy}
            onChange={(event) => updateMakeCopy(event.target.checked)}
          />
          <span className="recording-switch" aria-hidden="true" />
          <span>Save as new file</span>
        </label>
        <div className="recording-save-action-area">
          <div
            className={`recording-save-toast${error ? " error" : toast ? " success" : ""}${saveStatus ? "" : " empty"}`}
            aria-live={error ? "assertive" : "polite"}
          >
            {error
              ? <p role="alert">{error}</p>
              : saveStatus
                ? <p role={toast ? "status" : undefined}>{saveStatus}</p>
                : <span aria-hidden="true" />}
          </div>
          <div className="recording-save-actions">
            <button
              className={`recording-save-cancel${exportId ? "" : " is-placeholder"}`}
              type="button"
              disabled={!exportId}
              aria-hidden={!exportId}
              tabIndex={exportId ? 0 : -1}
              onClick={() => {
                if (exportId) void invoke("cancel_recording_export", { exportId });
              }}
            >Cancel</button>
            <button
              className={`recording-show-in-folder${exported && !exportId ? "" : " is-placeholder"}`}
              type="button"
              disabled={!exported || Boolean(exportId)}
              aria-hidden={!exported || Boolean(exportId)}
              tabIndex={exported && !exportId ? 0 : -1}
              onClick={() => void revealSavedRecording()}
            ><FolderIcon />Show in Folder</button>
            <button
              className="primary"
              type="button"
              aria-busy={Boolean(exportId)}
              disabled={Boolean(exportId) || alreadySaved}
              onClick={() => void startExport()}
            ><SaveIcon />Save</button>
          </div>
        </div>
      </footer>
    </main>
  );
}

function editorOutputDimensions(
  width: number,
  height: number,
  preset: "original" | "1080" | "720" | "custom",
  customWidth: number,
  customHeight: number,
): { width: number; height: number } {
  if (preset === "custom") return { width: evenDimension(customWidth), height: evenDimension(customHeight) };
  const maximum = preset === "1080" ? 1080 : preset === "720" ? 720 : height;
  const scale = height > maximum ? maximum / height : 1;
  return { width: evenDimension(width * scale), height: evenDimension(height * scale) };
}

function formatMaximumFileSizeInput(bytes: number, unit: FileSizeUnit): string {
  const value = bytes / FILE_SIZE_UNIT_BYTES[unit];
  return Number(value.toPrecision(8)).toString();
}

function dimensionsAtMaximumWidth(
  width: number,
  height: number,
  maximumWidth: number,
): { width: number; height: number } {
  if (width <= maximumWidth) return { width, height };
  const scale = maximumWidth / width;
  return { width: evenDimension(maximumWidth), height: evenDimension(height * scale) };
}

function boundedCrop(
  crop: { x: number; y: number; width: number; height: number },
  sourceWidth: number,
  sourceHeight: number,
): { x: number; y: number; width: number; height: number } {
  const x = clampNumber(Math.round(crop.x), 0, Math.max(0, sourceWidth - 2));
  const y = clampNumber(Math.round(crop.y), 0, Math.max(0, sourceHeight - 2));
  return {
    x,
    y,
    width: clampNumber(Math.round(crop.width), 2, Math.max(2, sourceWidth - x)),
    height: clampNumber(Math.round(crop.height), 2, Math.max(2, sourceHeight - y)),
  };
}

function clampNumber(value: number, minimum: number, maximum: number): number {
  if (!Number.isFinite(value)) return minimum;
  return Math.min(Math.max(value, minimum), Math.max(minimum, maximum));
}

function evenDimension(value: number): number {
  const rounded = Math.max(2, Math.round(value));
  return rounded % 2 === 0 ? rounded : rounded - 1;
}

function exportStageLabel(stage: ExportProgress["stage"] | undefined): string {
  if (stage === "preparing") return "Preparing…";
  if (stage === "encoding") return "Saving…";
  if (stage === "verifying") return "Checking file size…";
  if (stage === "cancelled") return "Save cancelled.";
  if (stage === "failed") return "Save failed.";
  return "Saved.";
}

function CaptureOverlay() {
  const [session, setSession] = useState<ActiveSession | null>(null);
  const [visibleSessionId, setVisibleSessionId] = useState<string | null>(null);
  const [primingSessionId, setPrimingSessionId] = useState<string | null>(null);
  const [start, setStart] = useState<SelectionPoint | null>(null);
  const [current, setCurrent] = useState<SelectionPoint | null>(null);
  const [regionForceSquare, setRegionForceSquare] = useState(false);
  const [hoveredWindow, setHoveredWindow] = useState<string | null>(null);
  const [hoveredDisplay, setHoveredDisplay] = useState(false);
  const [selectionFeedback, setSelectionFeedback] = useState(0);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const activeSessionIdRef = useRef<string | null>(null);
  const revealingSessionIdRef = useRef<string | null>(null);
  const overlayWakeRef = useRef<{
    sessionId: string;
    promise: Promise<unknown>;
  } | null>(null);
  const captureCancelledRef = useRef(false);
  const regionOverlayWarmedRef = useRef(false);
  const selectionFeedbackTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastRegionCursorSyncAtRef = useRef(0);
  const regionDragRef = useRef<{
    start: SelectionPoint;
    current: SelectionPoint;
    forceSquare: boolean;
  } | null>(null);
  const lastWindowPointerRef = useRef<SelectionPoint | null>(null);
  const windowHoverSurfaceRef = useRef<string | null>(null);
  const sessionId = session?.id ?? query("session_id");
  const mode = session?.mode ?? ((query("mode") ?? "region") as CaptureMode);

  useEffect(() => {
    let active = true;
    const cleanup = createCleanupRegistry();
    void (async () => {
      const unlisten = await listen<ActiveSession>("capture-session-ready", ({ payload }) => {
        if (!active) return;
        if (activeSessionIdRef.current === payload.id) {
          setSession(payload);
          return;
        }
        activeSessionIdRef.current = payload.id;
          revealingSessionIdRef.current = null;
          captureCancelledRef.current = false;
          setVisibleSessionId(null);
        setPrimingSessionId(null);
        setSession(payload);
        setStart(null);
        setCurrent(null);
        regionDragRef.current = null;
        lastWindowPointerRef.current = null;
        windowHoverSurfaceRef.current = null;
        setRegionForceSquare(false);
        setHoveredWindow(null);
        setHoveredDisplay(false);
        if (selectionFeedbackTimerRef.current) {
          clearTimeout(selectionFeedbackTimerRef.current);
          selectionFeedbackTimerRef.current = null;
        }
        setSelectionFeedback(0);
        lastRegionCursorSyncAtRef.current = 0;
      });
      if (!cleanup.add(unlisten)) return;
      const initialSession = query("session_id")
        ? await invoke<ActiveSession | null>("get_active_session", { sessionId: query("session_id") })
        : await invoke<ActiveSession | null>("get_pending_session");
      if (active && initialSession) {
        activeSessionIdRef.current = initialSession.id;
        revealingSessionIdRef.current = null;
        captureCancelledRef.current = false;
        setVisibleSessionId(null);
        setPrimingSessionId(null);
        setSession(initialSession);
      }
    })();
    return () => {
      active = false;
      cleanup.dispose();
      activeSessionIdRef.current = null;
      revealingSessionIdRef.current = null;
    };
  }, []);

  useEffect(() => () => {
    if (selectionFeedbackTimerRef.current) clearTimeout(selectionFeedbackTimerRef.current);
  }, []);

  useEffect(() => onCaptureEscape(() => {
    // Match the region commit fast path: hide the native surface before the
    // async command crosses into Rust. The backend repeats the hide while it
    // restores the previous app and capture UI, so this remains best-effort.
    captureCancelledRef.current = true;
    activeSessionIdRef.current = null;
    revealingSessionIdRef.current = null;
    overlayWakeRef.current = null;
    dismissCaptureOverlayWindow();
    if (sessionId) {
      void invoke("cancel_capture", { sessionId });
    } else {
      void invoke("cancel_active_capture");
    }
  }), [sessionId]);

  useEffect(() => {
    const onShift = (event: KeyboardEvent) => {
      if (event.key !== "Shift" || mode !== "region" || !regionDragRef.current) return;
      const forceSquare = event.type === "keydown";
      regionDragRef.current.forceSquare = forceSquare;
      setRegionForceSquare(forceSquare);
    };
    window.addEventListener("keydown", onShift, true);
    window.addEventListener("keyup", onShift, true);
    return () => {
      window.removeEventListener("keydown", onShift, true);
      window.removeEventListener("keyup", onShift, true);
    };
  }, [mode]);

  useEffect(() => {
    if (!sessionId) return;
    const cursorClass = `capture-${mode}-cursor`;
    document.documentElement.classList.add(cursorClass);
    return () => document.documentElement.classList.remove(cursorClass);
  }, [mode, sessionId]);

  const hoveredWindowLayout = useMemo(() => {
    if (mode !== "window" || !hoveredWindow || !session) return null;
    const match = session.windows.find((window) => window.id === hoveredWindow);
    if (!match) return null;
    const scale = session.window_coordinate_scale || 1;
    return {
      x: (match.x - session.display.x) / scale,
      y: (match.y - session.display.y) / scale,
      width: match.width / scale,
      height: match.height / scale,
      cornerRadius: match.corner_radius ?? session.window_corner_radius,
    };
  }, [hoveredWindow, mode, session]);

  // ALL hooks must stay above any early return.
  const windowLayouts = useMemo(() => {
    if (mode !== "window" || !session) return [];
    const scale = Math.max(session.window_coordinate_scale || 1, 1);
    // Native z-order is front-to-back; index 0 is topmost (highest z-index).
    return frontToBackWindows(
      session.windows.filter((window) => window.width >= 48 && window.height >= 48),
    )
      .map((window, index, list) => ({
        window,
        left: (window.x - session.display.x) / scale,
        top: (window.y - session.display.y) / scale,
        width: window.width / scale,
        height: window.height / scale,
        cornerRadius: window.corner_radius ?? session.window_corner_radius,
        zIndex: list.length - index,
      }));
  }, [mode, session]);

  const displayOverlay = session
    ? displayOverlaySize(session.display, session.window_coordinate_scale)
    : { width: 0, height: 0 };
  const surfaceSize = useElementCssSize(surfaceRef, displayOverlay);
  const rect = useMemo(
    () => (start && current
      ? dragSelectionRect(
          "create",
          start,
          current,
          { x: start.x, y: start.y, width: 0, height: 0 },
          surfaceSize,
          { forceSquare: regionForceSquare },
        )
      : null),
    [current, regionForceSquare, start, surfaceSize],
  );

  const overlayFrozen = sessionShowsFreezeFrame(session);

  const wakeOverlay = useCallback(() => {
    if (!sessionId || captureCancelledRef.current) return Promise.resolve();
    if (activeSessionIdRef.current && activeSessionIdRef.current !== sessionId) {
      return Promise.resolve();
    }
    const existing = overlayWakeRef.current;
    if (existing?.sessionId === sessionId) return existing.promise;
    const promise = invoke("show_capture_overlay", { sessionId }).catch((error) => {
      if (overlayWakeRef.current?.promise === promise) overlayWakeRef.current = null;
      throw error;
    });
    overlayWakeRef.current = { sessionId, promise };
    return promise;
  }, [sessionId]);

  const revealOverlay = useCallback(() => {
    if (!sessionId || captureCancelledRef.current) return;
    if (activeSessionIdRef.current !== sessionId) return;
    if (revealingSessionIdRef.current === sessionId) return;
    revealingSessionIdRef.current = sessionId;
    const shouldPrimeRegionOverlay = overlayFrozen && mode === "region" && !regionOverlayWarmedRef.current;
    void wakeOverlay().then(() => {
      if (captureCancelledRef.current || activeSessionIdRef.current !== sessionId) return;
      if (shouldPrimeRegionOverlay) {
        // Keep the shade at rest while the snapshot paints under native alpha.
        // The snapshot itself is always CSS-opaque; only the dim fades in after
        // reveal so live/frozen editor chrome never crossfades.
        setPrimingSessionId(sessionId);
      }
      let revealed = false;
      const finishReveal = () => {
        if (revealed || captureCancelledRef.current || activeSessionIdRef.current !== sessionId) return;
        revealed = true;
        window.clearTimeout(fallbackTimer);
        // Native reveal makes the already-painted snapshot fully opaque, then
        // focuses the overlay under cover of that frame (macOS). Fade only the
        // shade / chrome after that so open editors cannot shimmer.
        void invoke("reveal_capture_overlay", { sessionId }).then(() => {
          if (captureCancelledRef.current || activeSessionIdRef.current !== sessionId) return;
          void invoke("sync_capture_cursor", { sessionId });
          if (shouldPrimeRegionOverlay) regionOverlayWarmedRef.current = true;
          requestAnimationFrame(() => {
            if (captureCancelledRef.current || activeSessionIdRef.current !== sessionId) return;
            setPrimingSessionId(null);
            setVisibleSessionId(sessionId);
          });
        }).catch(() => {
          setPrimingSessionId(null);
          setVisibleSessionId(null);
          if (revealingSessionIdRef.current === sessionId) revealingSessionIdRef.current = null;
        });
      };
      afterNextPaint(finishReveal);
      // WebKit can suspend requestAnimationFrame at near-zero opacity. Always
      // reveal after a short deadline once the snapshot has asked to paint.
      const fallbackTimer = window.setTimeout(
        finishReveal,
        CAPTURE_OVERLAY_REVEAL_FALLBACK_MS,
      );
    }).catch(() => {
      if (revealingSessionIdRef.current === sessionId) revealingSessionIdRef.current = null;
    });
  }, [mode, overlayFrozen, sessionId, wakeOverlay]);

  // Wake the overlay as soon as a session exists so a hidden WKWebView will
  // load the snapshot. Do not reveal here — that used to race the image decode
  // and flash a black unpainted surface.
  useEffect(() => {
    if (!session?.id || captureCancelledRef.current) return;
    void wakeOverlay().catch(() => undefined);
  }, [session?.id, wakeOverlay]);

  // Safety: if snapshot onLoad never fires, still reveal so capture is not stuck.
  // Live overlays have no freeze-frame, so reveal as soon as the session exists.
  useEffect(() => {
    if (!session?.id || captureCancelledRef.current) return;
    if (!overlayFrozen) {
      void revealOverlay();
      return;
    }
    const timer = window.setTimeout(() => {
      void revealOverlay();
    }, CAPTURE_OVERLAY_REVEAL_FALLBACK_MS);
    return () => window.clearTimeout(timer);
  }, [overlayFrozen, session?.id, revealOverlay]);

  const applyWindowHoverAt = useCallback((point: SelectionPoint) => {
    if (!session || session.mode !== "window") return;
    lastWindowPointerRef.current = point;
    const hover = windowPointerHoverAtPoint(
      capturableOverlayWindows(session.windows),
      session.shell_chrome ?? [],
      point,
      session.display,
      Math.max(session.window_coordinate_scale || 1, 1),
      session.windows_ready,
    );
    setHoveredWindow(hover.windowId);
    setHoveredDisplay(hover.display);
  }, [session]);

  useEffect(() => {
    if (mode !== "window" || !session?.id) return;
    const surfaceKey = `${session.id}:${session.display.id}`;
    if (windowHoverSurfaceRef.current !== surfaceKey) {
      lastWindowPointerRef.current = null;
      windowHoverSurfaceRef.current = surfaceKey;
    }
    const existing = lastWindowPointerRef.current;
    if (existing) {
      applyWindowHoverAt(existing);
      return;
    }
    let cancelled = false;
    void requestCapturePointerPosition().then((pointer) => {
      if (cancelled || lastWindowPointerRef.current || !pointer?.inside) return;
      applyWindowHoverAt({ x: pointer.x, y: pointer.y });
    });
    return () => {
      cancelled = true;
    };
  }, [
    applyWindowHoverAt,
    mode,
    session?.id,
    session?.display.id,
    session?.windows,
    session?.windows_ready,
    visibleSessionId,
  ]);

  if (!session || !sessionId) {
    return <main className="capture-loading">Preparing capture…</main>;
  }

  const pointFromEvent = (event: React.PointerEvent) => overlayPointFromClient(
    surfaceRef.current,
    event.clientX,
    event.clientY,
  );

  const commitRegion = (selection: SelectionRect | null): boolean => {
    if (!isCapturableSelection(selection)) return false;
    // Tauri's built-in window command reaches the native window directly. Start
    // hiding on pointer release instead of waiting for the async capture command
    // to be scheduled and hop back to the platform UI thread first. The backend
    // repeats this hide before it touches pixels, so live captures remain safe if
    // this best-effort fast path has not completed yet.
    dismissCaptureOverlayWindow();
    void invoke("commit_region", { sessionId, rect: selection });
    return true;
  };

  const clearSelectionFeedback = () => {
    if (selectionFeedbackTimerRef.current) {
      clearTimeout(selectionFeedbackTimerRef.current);
      selectionFeedbackTimerRef.current = null;
    }
    setSelectionFeedback(0);
  };

  const showSelectionFeedback = () => {
    if (selectionFeedbackTimerRef.current) clearTimeout(selectionFeedbackTimerRef.current);
    setSelectionFeedback((attempt) => attempt + 1);
    selectionFeedbackTimerRef.current = setTimeout(() => {
      selectionFeedbackTimerRef.current = null;
      setSelectionFeedback(0);
    }, 1800);
  };

  const reassertRegionCursor = () => {
    const now = performance.now();
    const lastSyncAt = lastRegionCursorSyncAtRef.current;
    if (lastSyncAt !== 0 && now - lastSyncAt < 100) return;
    lastRegionCursorSyncAtRef.current = now;
    void invoke("sync_capture_cursor", { sessionId });
  };

  const onPointerDown = (event: React.PointerEvent) => {
    if (mode !== "region") return;
    event.preventDefault();
    clearSelectionFeedback();
    reassertRegionCursor();
    event.currentTarget.setPointerCapture(event.pointerId);
    const point = pointFromEvent(event);
    const forceSquare = event.shiftKey;
    regionDragRef.current = { start: point, current: point, forceSquare };
    setRegionForceSquare(forceSquare);
    setStart(point);
    setCurrent(point);
  };

  const onPointerMove = (event: React.PointerEvent) => {
    if (mode === "window") {
      applyWindowHoverAt(pointFromEvent(event));
      return;
    }
    if (mode !== "region") return;
    reassertRegionCursor();
    const drag = regionDragRef.current;
    if (!drag) return;
    const point = pointFromEvent(event);
    drag.current = point;
    drag.forceSquare = event.shiftKey;
    setRegionForceSquare(event.shiftKey);
    setCurrent(point);
  };

  const finishRegionDrag = (event: React.PointerEvent, commit: boolean) => {
    if (mode !== "region") return;
    const drag = regionDragRef.current;
    regionDragRef.current = null;
    if (
      typeof event.currentTarget.hasPointerCapture === "function"
      && event.currentTarget.hasPointerCapture(event.pointerId)
    ) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (!drag) {
      setStart(null);
      setCurrent(null);
      setRegionForceSquare(false);
      return;
    }
    const finalRect = dragSelectionRect(
      "create",
      drag.start,
      drag.current,
      { x: drag.start.x, y: drag.start.y, width: 0, height: 0 },
      surfaceSize,
      { forceSquare: event.shiftKey },
    );
    if (commit && !commitRegion(finalRect)) showSelectionFeedback();
    setStart(null);
    setCurrent(null);
    setRegionForceSquare(false);
  };

  const onPointerUp = (event: React.PointerEvent) => {
    if (mode === "window") {
      applyWindowHoverAt(pointFromEvent(event));
      const scale = Math.max(session.window_coordinate_scale || 1, 1);
      const hit = frontmostCaptureTargetAtPoint(
        capturableOverlayWindows(session.windows),
        session.shell_chrome ?? [],
        pointFromEvent(event),
        session.display,
        scale,
      );
      if (hit?.kind === "window") {
        dismissCaptureOverlayWindow();
        void invoke("commit_window", { sessionId, windowId: hit.target.id });
        return;
      }
      if (hit?.kind === "chrome" || windowListingIsReady(session.windows_ready)) {
        dismissCaptureOverlayWindow();
        void invoke("commit_display", { sessionId });
      }
      return;
    }
    finishRegionDrag(event, true);
  };

  const hasSelection = Boolean(rect && rect.width > 0 && rect.height > 0);
  const dimHole = mode === "region" && hasSelection && rect
    ? { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
    : mode === "window" && !hoveredDisplay
      ? hoveredWindowLayout
      : null;
  const displayCornerRadius = Math.max(0, session.display_corner_radius ?? 0);

  return (
    <main
      key={sessionId}
      ref={surfaceRef}
      className={`capture-surface capture-${mode}${visibleSessionId === sessionId ? " capture-visible" : ""}${primingSessionId === sessionId ? " capture-priming" : ""}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={(event) => finishRegionDrag(event, false)}
      onPointerEnter={(event) => {
        if (mode === "window") applyWindowHoverAt(pointFromEvent(event));
      }}
      onDoubleClick={(event) => event.preventDefault()}
      onDragStart={(event) => event.preventDefault()}
      onTransitionEnd={(event) => {
        const target = event.target;
        const finishedSurfaceFade = target === event.currentTarget;
        const finishedRegionShadeFade = target instanceof HTMLElement
          && target.classList.contains("capture-shade-full");
        if (
          event.propertyName === "opacity"
          && (finishedSurfaceFade || finishedRegionShadeFade)
        ) {
          reassertRegionCursor();
        }
      }}
    >
      {overlayFrozen && session.snapshot_url ? (
        <img
          className="capture-snapshot"
          src={session.snapshot_url}
          alt=""
          draggable={false}
          onLoad={() => void revealOverlay()}
          onError={() => void revealOverlay()}
        />
      ) : null}
      <CaptureDim
        mode={mode}
        hole={dimHole}
        bounds={surfaceSize}
        windowCornerRadius={hoveredWindowLayout?.cornerRadius ?? session.window_corner_radius}
      />
      <CaptureGuidance
        key={`${sessionId}-${selectionFeedback}`}
        mode={mode === "region" ? "region" : hoveredDisplay ? "display" : "window"}
        feedback={mode === "region" && selectionFeedback > 0}
        hidden={mode === "region" && Boolean(start)}
      />
      {mode === "window" && hoveredDisplay && (
        <>
          <div
            className="capture-display-outline"
            aria-hidden="true"
            style={displayCornerRadius > 0 ? { borderRadius: displayCornerRadius } : undefined}
          />
          <div className="capture-display-fallback" aria-hidden="true">
            <span>Entire display</span>
          </div>
        </>
      )}
      {hasSelection && rect && (
        <div
          className="selection-box"
          style={{
            left: rect.x,
            top: rect.y,
            width: rect.width,
            height: rect.height,
          }}
        >
          <span
            className="selection-dimensions"
            data-screen-edge={rect.y < 30 ? "top" : undefined}
          >
            {Math.round(rect.width)} × {Math.round(rect.height)}
          </span>
        </div>
      )}
      {mode === "window" && (
        <div className="window-targets">
          {windowLayouts.map((item) => (
            <button
              type="button"
              key={item.window.id}
              className={`window-target${hoveredWindow === item.window.id ? " window-target-hovered" : ""}`}
              style={{
                left: item.left,
                top: item.top,
                width: item.width,
                height: item.height,
                zIndex: item.zIndex,
                borderRadius: item.cornerRadius,
              }}
              title={item.window.title || item.window.app_name || "Window"}
            >
              <span>{item.window.title || item.window.app_name || "Window"}</span>
            </button>
          ))}
        </div>
      )}
    </main>
  );
}

/**
 * Soft dim with an optional rectangular hole.
 * Region mode reveals the already-painted snapshot cleanly, then fades this
 * shade on top. Window mode can stay clear until hover for screenshot capture,
 * or dim immediately while the recording selector waits for a window choice.
 *
 * Square holes use a CSS clip-path in the same CSS pixel space as the marquee
 * so Windows DPI cannot desync SVG viewBox units from pointer coordinates.
 * Rounded window holes still use an SVG path; `bounds` should be the live
 * surface client size so path units stay aligned.
 */
function CaptureDim({
  mode,
  hole,
  bounds,
  dimWithoutHole = false,
  windowCornerRadius = 0,
}: {
  mode: CaptureMode;
  hole: { x: number; y: number; width: number; height: number } | null;
  bounds: { width: number; height: number };
  dimWithoutHole?: boolean;
  windowCornerRadius?: number;
}) {
  // Screenshot window capture stays clear until hover. Recording selection can
  // opt into a full shade while it waits for the user to choose a window.
  if (mode === "window" && !hole && !dimWithoutHole) return null;

  if (!hole) {
    return <div className="capture-shade capture-shade-full" />;
  }

  const { x, y, width, height } = hole;
  const left = Math.max(0, x);
  const top = Math.max(0, y);
  const right = left + Math.max(0, width);
  const bottom = top + Math.max(0, height);
  const radius = mode === "window" ? Math.max(0, windowCornerRadius) : 0;

  if (radius === 0) {
    return (
      <div
        className="capture-shade capture-shade-full"
        style={{ clipPath: captureDimClipPath({ x: left, y: top, width: right - left, height: bottom - top }) }}
        aria-hidden="true"
      />
    );
  }

  const boxWidth = Math.max(bounds.width, right, 1);
  const boxHeight = Math.max(bounds.height, bottom, 1);
  const path = [
    `M0 0H${boxWidth}V${boxHeight}H0Z`,
    roundedRectPath(
      { x: left, y: top, width: right - left, height: bottom - top },
      radius,
    ),
  ].join(" ");
  return (
    <svg
      className="capture-shade-cutout"
      viewBox={`0 0 ${boxWidth} ${boxHeight}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      <path className="capture-shade capture-shade-path" d={path} fillRule="evenodd" />
    </svg>
  );
}

/** Live CSS client size of an element; falls back until layout is available. */
function useElementCssSize(
  ref: RefObject<HTMLElement | null>,
  fallback: { width: number; height: number },
): { width: number; height: number } {
  const [measured, setMeasured] = useState<{ width: number; height: number } | null>(null);

  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;

    const update = () => {
      const width = element.clientWidth;
      const height = element.clientHeight;
      if (width <= 0 || height <= 0) return;
      setMeasured((current) => (
        current && current.width === width && current.height === height
          ? current
          : { width, height }
      ));
    };

    update();
    if (typeof ResizeObserver !== "function") return;
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);

  return measured ?? fallback;
}

const STACK_MOTION_MS = THUMBNAIL_STACK_EXPAND_COLLAPSE_MS;

export function Thumbnail() {
  const [artifacts, setArtifacts] = useState<CaptureArtifact[]>([]);
  const [stackMotion, setStackMotion] = useState<
    "expanded" | "collapsing" | "collapsed" | "expanding"
  >("expanded");
  const [stackAnchor, setStackAnchor] = useState<ThumbnailStackAnchor>("bottom");
  const stackAnchorRef = useRef<ThumbnailStackAnchor>("bottom");
  const [stackSide, setStackSide] = useState<ThumbnailStackSide>("left");
  const stackSideRef = useRef<ThumbnailStackSide>("left");
  const placementRef = useRef<MiniPreviewPlacement>(DEFAULT_MINI_PREVIEW_PLACEMENT);
  const [stackHoverReady, setStackHoverReady] = useState(false);
  const [stackMinimizeRun, setStackMinimizeRun] = useState(false);
  const [stackHoverLatched, setStackHoverLatched] = useState(false);
  const [expandFromTransforms, setExpandFromTransforms] = useState<Map<string, string>>(
    () => new Map(),
  );
  const [exitingArtifactIds, setExitingArtifactIds] = useState<Set<string>>(
    () => new Set(),
  );
  const [clearingArtifactIds, setClearingArtifactIds] = useState<Set<string>>(
    () => new Set(),
  );
  const clearingArtifactIdsRef = useRef(clearingArtifactIds);
  const [clipboardState, setClipboardState] = useState<ClipboardState>({
    revision: -1,
    artifact_id: null,
  });
  const [activeViewerArtifactId, setActiveViewerArtifactId] = useState<string | null>(null);
  const [editorPresence, setEditorPresence] = useState<Map<string, string[]>>(
    () => new Map(),
  );
  const editorActiveArtifactIds = useMemo(
    () => artifactIdsInEditors(editorPresence),
    [editorPresence],
  );
  const [stackOverflow, setStackOverflow] = useState({
    hasOlder: false,
    hasNewer: false,
  });
  const [stackViewportHeight, setStackViewportHeight] = useState(() => (
    typeof window === "undefined" ? 0 : window.innerHeight
  ));
  const stackRef = useRef<HTMLElement>(null);
  const stackDrag = useRef<CollapsedThumbnailStackDrag | null>(null);
  // Native collapsed windows retain their expanded height. Track which end
  // currently owns the compact pile so a live midpoint flip can convert that
  // tall frame without moving the visible previews.
  const collapsedLayoutAnchorRef = useRef<ThumbnailStackAnchor>("bottom");
  const collapsedStackPointerCleanup = useRef<(() => void) | null>(null);
  const skipCollapsedStackClick = useRef(false);
  const stackFanCollapseTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const stackFanCollapsed = useRef(false);
  const stackMotionTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const stackHoverReadyFrames = useRef<{ first: number; second: number } | null>(null);
  const previousStackMotion = useRef<"expanded" | "collapsing" | "collapsed" | "expanding">(
    "expanded",
  );
  const previousArtifactCount = useRef(0);
  const pendingNewestReveal = useRef(false);
  const cancelStackScroll = useRef<(() => void) | null>(null);
  const applyClipboardState = useCallback((next: ClipboardState) => {
    setClipboardState((current) => reconcileClipboardState(current, next));
  }, []);
  const setArtifactExiting = useCallback((artifactId: string, exiting: boolean) => {
    setExitingArtifactIds((current) => {
      if (current.has(artifactId) === exiting) return current;
      const next = new Set(current);
      if (exiting) next.add(artifactId);
      else next.delete(artifactId);
      return next;
    });
  }, []);
  const replaceClearingArtifactIds = useCallback((next: Set<string>) => {
    clearingArtifactIdsRef.current = next;
    setClearingArtifactIds(next);
  }, []);
  const refreshStackOverflow = useCallback(() => {
    const stack = stackRef.current;
    if (!stack) return;
    // Use layout geometry, not scrollHeight: dust chips and settle transforms
    // inflate WebKit paint overflow and flash the bottom "newer" cue mid-exit.
    const cardCount = stack.querySelectorAll(":scope > .thumbnail-card").length;
    const next = thumbnailStackOverflow(
      stack.scrollTop,
      thumbnailStackContentHeight(cardCount),
      stack.clientHeight,
    );
    setStackOverflow((current) => (
      current.hasOlder === next.hasOlder && current.hasNewer === next.hasNewer
        ? current
        : next
    ));
  }, []);

  const commitStackAnchor = useCallback((nextAnchor: ThumbnailStackAnchor) => {
    if (stackAnchorRef.current === nextAnchor) return;
    stackAnchorRef.current = nextAnchor;
    setStackAnchor(nextAnchor);
    stackRef.current?.classList.toggle("thumbnail-stack-anchor-top", nextAnchor === "top");
    pendingNewestReveal.current = true;
  }, []);

  const commitStackSide = useCallback((nextSide: ThumbnailStackSide) => {
    if (stackSideRef.current === nextSide) return;
    stackSideRef.current = nextSide;
    setStackSide(nextSide);
  }, []);

  const applyMiniPreviewHome = useCallback((placement: MiniPreviewPlacement) => {
    placementRef.current = placement;
    const nextAnchor = thumbnailStackAnchorFromPlacement(placement);
    commitStackAnchor(nextAnchor);
    commitStackSide(thumbnailStackSideFromPlacement(placement));
    applyThumbnailStackGravity(
      stackRef.current,
      thumbnailStackGravityFromPlacement(placement),
    );
    if (isTauri()) return;
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    const home = harnessOffsetForPlacement(placement, viewport);
    writeHarnessStackOffset(home.x, home.y, document.documentElement, viewport, {
      anchor: home.anchor,
    });
  }, [commitStackAnchor, commitStackSide]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    let settingsChanged = false;
    const applySettings = (settings: AppSettings | null | undefined, forceHome: boolean) => {
      const placement = settings?.mini_preview_placement ?? DEFAULT_MINI_PREVIEW_PLACEMENT;
      if (!forceHome && placement === placementRef.current) return;
      applyMiniPreviewHome(placement);
    };
    void invoke<AppSettings>("get_settings")
      .then((settings) => {
        if (active && !settingsChanged) applySettings(settings, true);
      })
      .catch(() => undefined);
    void listen<AppSettings>("settings-changed", ({ payload }) => {
      settingsChanged = true;
      if (active) applySettings(payload, false);
    }).then((dispose) => {
      if (active) unlisten = dispose;
      else dispose();
    }).catch(() => undefined);
    return () => {
      active = false;
      unlisten?.();
    };
  }, [applyMiniPreviewHome]);

  useEffect(() => {
    let active = true;
    const cleanup = createCleanupRegistry();
    void (async () => {
      const removedArtifactIds = new Set<string>();
      const listeners = await Promise.all([
        listen<CaptureArtifact>("capture-completed", ({ payload }) => {
          if (!active) return;
          removedArtifactIds.delete(payload.id);
          setArtifactExiting(payload.id, false);
          const clearing = clearingArtifactIdsRef.current;
          if (clearing.size > 0) {
            // A new capture must not inherit a stuck Clear all lock. Drop any
            // still-leaving cards so the dock comes back interactive.
            replaceClearingArtifactIds(new Set());
            setExitingArtifactIds((current) => {
              if (current.size === 0) return current;
              const next = new Set(current);
              for (const id of clearing) next.delete(id);
              next.delete(payload.id);
              return next.size === current.size ? current : next;
            });
          }
          setArtifacts((current) => {
            const withoutCleared = clearing.size === 0
              ? current
              : current.filter(({ id }) => !clearing.has(id));
            return withoutCleared.some(({ id }) => id === payload.id)
              ? withoutCleared
              : [...withoutCleared, payload];
          });
        }),
        listen<CaptureArtifact>("artifact-updated", ({ payload }) => {
          if (!active) return;
          setArtifacts((current) => current.map((artifact) => artifact.id === payload.id ? payload : artifact));
        }),
        listen<string>("artifact-removed", ({ payload }) => {
          if (!active) return;
          if (clearingArtifactIdsRef.current.has(payload)) {
            // Clear all already took these off the backend stack. Keep the
            // cards mounted so the shared Close streak can finish; onRemoved
            // drops them after animationend.
            setActiveViewerArtifactId((current) => current === payload ? null : current);
            return;
          }
          removedArtifactIds.add(payload);
          setArtifactExiting(payload, false);
          setArtifacts((current) => current.filter(({ id }) => id !== payload));
          setActiveViewerArtifactId((current) => current === payload ? null : current);
        }),
        listen<ClipboardState>("clipboard-owner-changed", ({ payload }) => {
          if (!active) return;
          applyClipboardState(payload);
        }),
        listen<ViewerActivationState>("viewer-activation-changed", ({ payload }) => {
          if (!active) return;
          setActiveViewerArtifactId((current) => reconcileActiveViewer(current, payload));
        }),
        listen<EditorLayerPresence>("editor-layers-changed", ({ payload }) => {
          if (!active) return;
          setEditorPresence((current) => reconcileEditorPresence(current, payload));
        }),
      ]);
      if (!cleanup.add(...listeners)) return;
      const initialArtifacts = await invoke<CaptureArtifact[]>("get_artifacts");
      if (active) {
        setArtifacts((current) => {
          const merged = new Map(
            initialArtifacts
              .filter(({ id }) => !removedArtifactIds.has(id))
              .map((artifact) => [artifact.id, artifact]),
          );
          current.forEach((artifact) => merged.set(artifact.id, artifact));
          return [...merged.values()];
        });
      }
    })();
    return () => {
      active = false;
      cleanup.dispose();
    };
  }, [applyClipboardState, replaceClearingArtifactIds, setArtifactExiting]);

  useEffect(() => {
    // The thumbnail window is a drag source, never a file-drop destination.
    // Reject inbound drags explicitly so WebKit cannot navigate to a dropped
    // screenshot and replace the preview UI.
    const rejectInboundDrag = (event: DragEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.dataTransfer) event.dataTransfer.dropEffect = "none";
    };

    document.addEventListener("dragenter", rejectInboundDrag, true);
    document.addEventListener("dragover", rejectInboundDrag, true);
    document.addEventListener("drop", rejectInboundDrag, true);
    return () => {
      document.removeEventListener("dragenter", rejectInboundDrag, true);
      document.removeEventListener("dragover", rejectInboundDrag, true);
      document.removeEventListener("drop", rejectInboundDrag, true);
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let polling = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const schedulePoll = (delay: number) => {
      if (cancelled) return;
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        void poll();
      }, delay);
    };

    const poll = async () => {
      if (cancelled || polling) return;
      polling = true;
      try {
        const current = await invoke<ClipboardState>("get_clipboard_state");
        if (!cancelled) applyClipboardState(current);
      } catch {
        // Preserve the last known state if the platform clipboard is briefly unavailable.
      } finally {
        polling = false;
        schedulePoll(document.hidden ? 1_000 : 400);
      }
    };

    const pollImmediately = () => schedulePoll(0);
    document.addEventListener("visibilitychange", pollImmediately);
    window.addEventListener("focus", pollImmediately);
    schedulePoll(0);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      document.removeEventListener("visibilitychange", pollImmediately);
      window.removeEventListener("focus", pollImmediately);
    };
  }, [applyClipboardState]);

  useLayoutEffect(() => {
    const shouldReveal = shouldScrollThumbnailStackToEnd(
      previousArtifactCount.current,
      artifacts.length,
    );
    previousArtifactCount.current = artifacts.length;
    const fromTop = stackAnchorRef.current === "top";
    if (shouldReveal && stackRef.current) {
      scrollThumbnailStackToNewest(stackRef.current, {
        viewportHeight: stackViewportHeight,
        fromTop,
      });
    }
    refreshStackOverflow();
    let cancelled = false;
    // Sync may grow the native window for new cards. It intentionally does not
    // shrink after dismissals — that recomposes WKWebView and flickers survivors.
    if (clearingArtifactIds.size > 0) {
      // Clear all already drained the backend stack. Keep this frame until the
      // last Close streak finishes so the window is not hidden mid-animation.
      return () => {
        cancelled = true;
      };
    }
    void invoke("sync_thumbnail_stack")
      .catch(() => undefined)
      .finally(() => {
        if (!cancelled) {
          if (shouldReveal && stackRef.current) {
            scrollThumbnailStackToNewest(stackRef.current, {
              viewportHeight: stackViewportHeight,
              fromTop,
            });
          }
          refreshStackOverflow();
          window.dispatchEvent(new Event("captures-thumbnail-layout-changed"));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [artifacts.length, clearingArtifactIds.size, refreshStackOverflow, stackViewportHeight]);

  useLayoutEffect(() => {
    if (stackMotion !== "expanded" || !pendingNewestReveal.current) return;
    const stack = stackRef.current;
    if (!stack) return;
    const cancelReveal = scheduleScrollThumbnailStackToNewest(stack, {
      onScrolled: refreshStackOverflow,
      retryMs: 450,
      viewportHeight: stackViewportHeight,
      fromTop: stackAnchorRef.current === "top",
    });
    const finish = window.setTimeout(() => {
      pendingNewestReveal.current = false;
    }, 450);
    return () => {
      cancelReveal();
      window.clearTimeout(finish);
    };
  }, [stackMotion, refreshStackOverflow, stackViewportHeight, stackAnchor]);

  useEffect(() => {
    const refresh = () => {
      setStackViewportHeight(window.innerHeight);
      refreshStackOverflow();
      if (isTauri()) return;
      const offset = readHarnessStackOffset();
      const gravity = thumbnailStackGravityFromHarness({
        offsetY: offset.y,
        anchor: stackAnchorRef.current,
        viewportHeight: window.innerHeight,
        contentHeight: thumbnailCollapsedFrameHeight(Math.max(
          stackRef.current?.querySelectorAll(":scope > .thumbnail-card").length ?? 1,
          1,
        )),
      });
      applyThumbnailStackGravity(stackRef.current, gravity);
    };
    window.addEventListener("resize", refresh);
    window.addEventListener("captures-thumbnail-ready", refresh);
    window.addEventListener("captures-thumbnail-layout-changed", refresh);
    refresh();
    return () => {
      window.removeEventListener("resize", refresh);
      window.removeEventListener("captures-thumbnail-ready", refresh);
      window.removeEventListener("captures-thumbnail-layout-changed", refresh);
    };
  }, [artifacts.length, refreshStackOverflow]);

  const hasThumbnailCards = artifacts.length > 0;

  useEffect(() => {
    // Dust-delete and dismiss both hold layout; survivors slide toward the
    // stack anchor by N slots with the same ease. Pure CSS only moved one
    // fixed step (or reflowed flex on dismiss), which jittered multi-exit
    // batches.
    if (!hasThumbnailCards) return;
    const stack = stackRef.current;
    if (!stack) return;
    return createThumbnailStackShiftController(stack);
  }, [hasThumbnailCards]);

  useEffect(() => {
    // The macOS panel stays ordered onscreen at zero alpha when the stack is
    // empty so AppKit cannot transfer focus to an open editor. Do not leave the
    // pointer poll running against that transparent, click-through WebView.
    if (!hasThumbnailCards) return;
    // Keep one native hover tracker for the lifetime of the thumbnail window.
    // Restart only when the stack crosses between empty and non-empty; ordinary
    // card additions/removals preserve hover presentation and native cursors.
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let polling = false;
    let pointerPollGeneration = 0;
    let cursorKind: ThumbnailCursorKind = "default";
    let ignoringCursorEvents = false;
    let ignoreCursorEventsSynced = false;
    let ignoreCursorUpdate: Promise<void> = Promise.resolve();
    let lastCursorSyncAt = 0;
    let consecutiveNullPolls = 0;
    let pointerPollSupported = true;
    let cursorHandoffTimers: ReturnType<typeof setTimeout>[] = [];
    let cardHoverLocked = false;
    let cardHoverLockOrigin: { x: number; y: number } | null = null;
    /**
     * Clicks and document-window handoffs can make macOS restore the frontmost
     * app's arrow for a frame. Reassert the current interactive cursor
     * immediately while keeping the thumbnail panel nonactivating.
     */
    const reassertInteractiveCursor = () => {
      if (cursorKind === "default") return;
      setThumbnailCursor(cursorKind, { force: true });
    };

    const clearCursorHandoffTimers = () => {
      for (const timer of cursorHandoffTimers) clearTimeout(timer);
      cursorHandoffTimers = [];
    };

    /**
     * AppKit and WebKit install the arrow during mouse up/down and again when
     * Edit moves key-window focus to the editor. Reassert now and on a short
     * delay schedule so the hand wins both transitions without a pointer poll.
     */
    const preserveInteractiveCursorAcrossHandoff = () => {
      reassertInteractiveCursor();
      clearCursorHandoffTimers();
      cursorHandoffTimers = THUMBNAIL_CURSOR_HANDOFF_REASSERT_DELAYS_MS.map((delay) => (
        setTimeout(() => {
          reassertInteractiveCursor();
        }, delay)
      ));
    };

    const setThumbnailCursor = (
      kind: ThumbnailCursorKind,
      options: { force?: boolean } = {},
    ) => {
      // Only touch CSS when the hit-tested kind changes. Rewriting style.cursor
      // every 40ms poll made WebKit re-evaluate cursor rectangles and flash the
      // default arrow between AppKit grab/pointer updates.
      if (kind !== cursorKind) {
        applyThumbnailCssCursor(kind);
      }
      const now = performance.now();
      const action = thumbnailCursorSyncAction(
        cursorKind,
        kind,
        now - lastCursorSyncAt,
        options,
      );
      if (!action) return;
      const becameInteractive = cursorKind === "default" && kind !== "default";
      cursorKind = kind;
      lastCursorSyncAt = now;
      if (action === "reassert") {
        void invoke("reassert_thumbnail_cursor", { kind });
      } else {
        void invoke("set_thumbnail_cursor", { kind });
      }
      // First entry onto the drag source often leaves the pointer stationary.
      // Reassert on the next task so grab/pointer survive WebKit's cursor-rect
      // update without requiring a detour over a button.
      if (becameInteractive) {
        preserveInteractiveCursorAcrossHandoff();
      }
    };

    const setIgnoreCursorEvents = (ignore: boolean, force = false) => {
      // A restarted tracker does not know the native state left by its prior
      // lifetime. Always send the first sample, even when it matches the local
      // default, then deduplicate ordinary polling updates.
      if (!force && ignoreCursorEventsSynced && ignoringCursorEvents === ignore) return;
      ignoringCursorEvents = ignore;
      ignoreCursorEventsSynced = true;
      // After dismiss the window may stay tall; empty space above the stack
      // must pass clicks through so it does not block the desktop.
      // Serialize whole-window hit-test updates. A delayed `true` from an old
      // pointer sample must never land after deletion has re-armed survivors.
      ignoreCursorUpdate = ignoreCursorUpdate.then(async () => {
        await invoke("set_thumbnail_ignore_cursor_events", { ignore })
          .catch(() => undefined);
      });
    };

    const applyStackClickThrough = () => {
      // Last remaining cards keep their layout slot for the dissolve / dismiss
      // animation. Without this, Windows/Linux leave the always-on-top window
      // hit-testable for the whole ~3s delete because pointer polls used to
      // return null on those platforms.
      //
      // A live card somewhere in a preserved-height window must not make the
      // empty chrome eat desktop input. Prefer pass-through until a poll proves
      // the pointer is on a card. An in-progress pile drag is the exception.
      const dragging = Boolean(document.querySelector(".thumbnail-stack-dragging"));
      setIgnoreCursorEvents(
        thumbnailUnknownPointerShouldIgnoreCursorEvents(dragging, pointerPollSupported)
          || !thumbnailStackHasLiveHitTarget(),
        true,
      );
    };

    const invalidatePointerPoll = () => {
      pointerPollGeneration += 1;
      polling = false;
      if (timer) {
        clearTimeout(timer);
        timer = null;
      }
    };

    const clearNativeClasses = () => {
      clearThumbnailNativeHover();
    };

    const clearNativeHover = () => {
      clearNativeClasses();
      setThumbnailCursor("default");
    };

    const stopNativeTracking = () => {
      document.documentElement.classList.remove("thumbnail-native-tracking");
      clearNativeHover();
      clearThumbnailCssCursor();
      setIgnoreCursorEvents(true, true);
    };

    const lockCardHover = () => {
      cardHoverLocked = true;
      cardHoverLockOrigin = null;
      setThumbnailCardHoverSuppressed(true);
    };

    const unlockCardHover = () => {
      if (!cardHoverLocked) return;
      cardHoverLocked = false;
      cardHoverLockOrigin = null;
      setThumbnailCardHoverSuppressed(false);
    };

    const maybeUnlockCardHover = (
      position: ThumbnailPointerPosition,
      options: { fromPointerMove?: boolean } = {},
    ) => {
      if (!cardHoverLocked) return;
      // Window-relative coordinates change while the pile is still compact.
      // Capture the origin only after cards are in their expanded layout.
      if (thumbnailStackHoldsCollapsedPose()) return;
      if (options.fromPointerMove && !cardHoverLockOrigin) {
        unlockCardHover();
        return;
      }
      if (!cardHoverLockOrigin) {
        if (position.inside) {
          cardHoverLockOrigin = { x: position.x, y: position.y };
        } else {
          unlockCardHover();
        }
        return;
      }
      if (thumbnailCardHoverLockReleased(cardHoverLockOrigin, position)) {
        unlockCardHover();
      }
    };

    const applyNativeHover = (
      position: ThumbnailPointerPosition,
      options: { updateHitTest?: boolean } = {},
    ) => {
      maybeUnlockCardHover(position);
      const ignore = shouldIgnoreThumbnailCursorEvents(position);
      const kind = applyThumbnailNativeHover(position);
      if (ignore || kind === "default") {
        // Empty space after collapse still sits inside the native window.
        // Document-wide native cursor rules would keep the arrow over apps
        // that already receive the clicks.
        document.documentElement.classList.remove("thumbnail-native-tracking");
      } else {
        document.documentElement.classList.add("thumbnail-native-tracking");
      }
      // DOM hover can fire over the hole in the always-on-top window. Toggling
      // click-through from those events leaves Wayland (null pointer polls)
      // unable to restore hits: the window ignores the cursor, so no later
      // pointermove can undo it. Native samples still own hit testing.
      if (options.updateHitTest !== false) {
        setIgnoreCursorEvents(ignore);
      }
      setThumbnailCursor(kind);
    };

    const schedulePoll = (delay: number) => {
      if (cancelled) return;
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        void poll();
      }, delay);
    };

    /**
     * After sleep/resume the WebView can keep click-through, drop always-on-top,
     * or leave a hung pointer poll. Force the stack interactive again and fall
     * back to CSS :hover until native samples resume.
     */
    const recoverInteractivity = (refreshNative = true) => {
      consecutiveNullPolls = 0;
      invalidatePointerPoll();
      // Drop native-only presentation so CSS hover works while we re-arm.
      document.documentElement.classList.remove("thumbnail-native-tracking");
      clearNativeClasses();
      clearThumbnailCssCursor();
      cursorKind = "default";
      const dragging = Boolean(document.querySelector(".thumbnail-stack-dragging"));
      if (!thumbnailStackHasLiveHitTarget()) {
        // Exiting-only stacks must stay click-through. Re-arming here would
        // undo the Close/Delete pass-through for ~3s on Windows/Linux.
        setIgnoreCursorEvents(true, true);
        schedulePoll(0);
        return;
      }
      // Prefer pass-through until a poll proves the pointer is on a live card.
      // Forcing the whole tall window hit-testable covers apps underneath.
      setIgnoreCursorEvents(
        thumbnailUnknownPointerShouldIgnoreCursorEvents(dragging, pointerPollSupported),
        true,
      );
      if (refreshNative) {
        void invoke("refresh_thumbnail_interactivity").catch(() => undefined);
      }
      schedulePoll(0);
    };

    const poll = async () => {
      if (cancelled || polling) return;
      const generation = pointerPollGeneration;
      polling = true;
      let delay = 250;
      let recovered = false;
      try {
        // Timeout so a hung IPC after sleep cannot leave `polling` stuck true.
        const position = await withThumbnailPointerTimeout(
          invoke<ThumbnailPointerPosition | null>("get_thumbnail_pointer_position"),
        );
        if (cancelled || generation !== pointerPollGeneration) return;
        if (!position) {
          consecutiveNullPolls += 1;
          if (!thumbnailStackHasLiveHitTarget()) {
            setIgnoreCursorEvents(true);
            delay = 40;
          } else {
            // Wayland (and hung IPC) can still return null. Recover only when
            // the tall window is still eating desktop events, or native hover
            // tracking is stuck. Click-through with an unknown pointer is safe
            // and must not loop recover. Exiting stacks stay click-through.
            const needsRecovery = thumbnailNullPollNeedsDesktopInputRecovery(
              ignoringCursorEvents,
              document.documentElement.classList.contains("thumbnail-native-tracking"),
              pointerPollSupported,
            );
            if (
              needsRecovery
              && shouldRecoverThumbnailAfterNullPolls(consecutiveNullPolls)
            ) {
              recovered = true;
              recoverInteractivity();
              return;
            }
            // A focus handoff can briefly make the native pointer query
            // unavailable. Preserve the last presentation until a real sample
            // confirms that the pointer moved away so the card cannot flash.
            delay = 40;
          }
        } else {
          consecutiveNullPolls = 0;
          applyNativeHover(position);
          delay = 40;
        }
      } catch {
        if (cancelled || generation !== pointerPollGeneration) return;
        consecutiveNullPolls += 1;
        if (!thumbnailStackHasLiveHitTarget()) {
          setIgnoreCursorEvents(true);
          delay = 40;
        } else {
          const needsRecovery = thumbnailNullPollNeedsDesktopInputRecovery(
            ignoringCursorEvents,
            document.documentElement.classList.contains("thumbnail-native-tracking"),
            pointerPollSupported,
          );
          if (
            needsRecovery
            && shouldRecoverThumbnailAfterNullPolls(consecutiveNullPolls)
          ) {
            recovered = true;
            recoverInteractivity();
            return;
          }
          delay = 40;
        }
      } finally {
        if (!cancelled && generation === pointerPollGeneration) {
          polling = false;
          // recoverInteractivity already scheduled the next poll.
          if (!recovered) schedulePoll(delay);
        }
      }
    };

    const resumeFromSuspension = () => {
      if (document.hidden) return;
      recoverInteractivity();
    };

    const resumeFromNativeShow = () => {
      if (document.hidden) return;
      // The native command already restored the window and z-order. Reset only
      // the WebView-side polling state so this event cannot recursively invoke
      // refresh_thumbnail_interactivity.
      recoverInteractivity(false);
    };

    const pollImmediately = () => {
      // Invalidate the in-flight sample even when the WebView is currently
      // hidden. It may describe a slot that React is about to remove.
      invalidatePointerPoll();
      if (document.hidden) return;
      // Focus handoffs restore the frontmost app's arrow; reassert first so
      // the pointer/grab affordance does not wait for the next throttle window.
      preserveInteractiveCursorAcrossHandoff();
      schedulePoll(0);
    };

    const updateThumbnailHitTest = (event: Event) => {
      const detail = event instanceof CustomEvent
        ? (event as CustomEvent<{
          stackMotion?: string;
          previousStackMotion?: string;
        }>).detail
        : undefined;
      if (shouldLockThumbnailCardHoverOnStackMotion(
        detail?.stackMotion,
        detail?.previousStackMotion,
      )) {
        lockCardHover();
      }
      clearNativeHover();
      const applyHitTest = () => {
        // Wait a microtask so React can commit `.thumbnail-exiting` from the
        // Close/Delete click before we decide whether the window must pass
        // clicks through. Re-arming first left Windows/Linux blocking the
        // desktop for the whole exit animation.
        applyStackClickThrough();
        pollImmediately();
      };
      queueMicrotask(applyHitTest);
    };

    const onPointerMove = (event: PointerEvent) => {
      if (cardHoverLocked) {
        const wasLocked = cardHoverLocked;
        maybeUnlockCardHover(
          { x: event.clientX, y: event.clientY, inside: true },
          { fromPointerMove: true },
        );
        if (wasLocked && !cardHoverLocked) pollImmediately();
      }
      // Native pointer polls cover click-through macOS panels. DOM moves cover
      // the harness and Windows/Linux WebViews, where glow :hover already
      // fires but CSS cursor often stays the arrow until mousedown.
      if (event.pointerType !== "touch") {
        applyNativeHover(
          {
            x: event.clientX,
            y: event.clientY,
            inside: true,
          },
          { updateHitTest: false },
        );
      }
    };

    const onPointerLeaveWindow = (event: PointerEvent) => {
      if (event.relatedTarget) return;
      applyNativeHover(
        { x: event.clientX, y: event.clientY, inside: false },
        { updateHitTest: false },
      );
    };

    const onPointerActivity = (event: Event) => {
      // Only primary-button presses/releases reset the AppKit cursor on macOS.
      if (event instanceof PointerEvent) {
        if (event.pointerType === "mouse" && event.button !== 0) return;
      } else if (event instanceof MouseEvent && event.button !== 0) {
        return;
      }
      preserveInteractiveCursorAcrossHandoff();
    };

    document.addEventListener("visibilitychange", resumeFromSuspension);
    // Keep this recovery path for runtime/platform focus notifications. The
    // thumbnail is nonactivating, but a full-size viewer or resumed WebView can
    // still trigger reconciliation without flashing the last hover state.
    window.addEventListener("focus", pollImmediately);
    // Opening an editor, dialog, or external folder moves key-window focus
    // away after the click handlers run. Preserve the hovered button's cursor
    // through that later native transition as well.
    window.addEventListener("blur", preserveInteractiveCursorAcrossHandoff);
    // Capture-phase so we reassert before WebKit's own cursor update from the click.
    window.addEventListener("pointermove", onPointerMove, true);
    window.addEventListener("pointerleave", onPointerLeaveWindow, true);
    window.addEventListener("pointerdown", onPointerActivity, true);
    window.addEventListener("pointerup", onPointerActivity, true);
    // `click` fires after mouseup and after the Edit handler starts opening the
    // editor window — cover that later AppKit handoff as well.
    window.addEventListener("click", onPointerActivity, true);
    window.addEventListener("pageshow", resumeFromSuspension);
    window.addEventListener("online", resumeFromSuspension);
    window.addEventListener("captures-thumbnail-resumed", resumeFromNativeShow);
    document.addEventListener("resume", resumeFromSuspension as EventListener);
    window.addEventListener("captures-thumbnail-ready", pollImmediately);
    window.addEventListener("captures-thumbnail-layout-changed", pollImmediately);
    window.addEventListener(
      THUMBNAIL_HIT_TEST_CHANGED_EVENT,
      updateThumbnailHitTest,
    );
    void invoke<boolean>("thumbnail_pointer_poll_available")
      .then((available) => {
        if (cancelled) return;
        pointerPollSupported = available !== false;
        applyStackClickThrough();
      })
      .catch(() => undefined);
    schedulePoll(0);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      clearCursorHandoffTimers();
      document.removeEventListener("visibilitychange", resumeFromSuspension);
      window.removeEventListener("focus", pollImmediately);
      window.removeEventListener("blur", preserveInteractiveCursorAcrossHandoff);
      window.removeEventListener("pointermove", onPointerMove, true);
      window.removeEventListener("pointerleave", onPointerLeaveWindow, true);
      window.removeEventListener("pointerdown", onPointerActivity, true);
      window.removeEventListener("pointerup", onPointerActivity, true);
      window.removeEventListener("click", onPointerActivity, true);
      window.removeEventListener("pageshow", resumeFromSuspension);
      window.removeEventListener("online", resumeFromSuspension);
      window.removeEventListener("captures-thumbnail-resumed", resumeFromNativeShow);
      document.removeEventListener("resume", resumeFromSuspension as EventListener);
      window.removeEventListener("captures-thumbnail-ready", pollImmediately);
      window.removeEventListener("captures-thumbnail-layout-changed", pollImmediately);
      window.removeEventListener(
        THUMBNAIL_HIT_TEST_CHANGED_EVENT,
        updateThumbnailHitTest,
      );
      stopNativeTracking();
      unlockCardHover();
    };
  }, [hasThumbnailCards]);

  useEffect(() => {
    return () => {
      collapsedStackPointerCleanup.current?.();
      cancelStackScroll.current?.();
      cancelStackScroll.current = null;
      if (stackMotionTimer.current) clearTimeout(stackMotionTimer.current);
      if (stackFanCollapseTimer.current) {
        clearTimeout(stackFanCollapseTimer.current);
        stackFanCollapseTimer.current = null;
      }
      if (stackHoverReadyFrames.current) {
        cancelAnimationFrame(stackHoverReadyFrames.current.first);
        cancelAnimationFrame(stackHoverReadyFrames.current.second);
        stackHoverReadyFrames.current = null;
      }
    };
  }, []);

  useEffect(() => {
    const previous = previousStackMotion.current;
    previousStackMotion.current = stackMotion;
    window.dispatchEvent(new CustomEvent(THUMBNAIL_HIT_TEST_CHANGED_EVENT, {
      detail: { stackMotion, previousStackMotion: previous },
    }));
  }, [stackMotion]);

  useEffect(() => {
    if (stackMotion === "expanded") return;
    const blockHtml5Drag = (event: DragEvent) => {
      preventThumbnailHtml5Drag(event);
    };
    document.addEventListener("dragstart", blockHtml5Drag, true);
    return () => document.removeEventListener("dragstart", blockHtml5Drag, true);
  }, [stackMotion]);

  if (artifacts.length === 0) return null;

  const collapsed = stackMotion === "collapsed";
  const compact = stackMotion !== "expanded";
  const rejectQuery = query("reject");
  const previewRejectShake = !compact
    && rejectQuery !== null
    && rejectQuery !== "0"
    && rejectQuery !== "false";
  const stackAnimating = stackMotion === "collapsing" || stackMotion === "expanding";
  const exitingOnly = artifacts.every(({ id }) => exitingArtifactIds.has(id));
  const livePreviewCount = artifacts.reduce(
    (count, artifact) => (exitingArtifactIds.has(artifact.id) ? count : count + 1),
    0,
  );
  // Clear all is the stack equivalent of Close: leave files and history, hide
  // the dock. One preview already has Close/Delete, so this stays off a
  // single card and off the collapsed pile.
  const showClearAll = !collapsed && livePreviewCount >= 2;
  const stackClearing = clearingArtifactIds.size > 0;
  const controlsDisabled = stackAnimating || exitingOnly || stackClearing;
  const stackScrollport = (stackMotion === "expanding" || stackMotion === "expanded")
    && thumbnailStackNeedsScrollport(artifacts.length, stackViewportHeight);
  const showOverflowCues = stackMotion === "expanded" && !stackClearing;

  const clearAllPreviews = () => {
    if (controlsDisabled || livePreviewCount < 2) return;
    // Skip cards already exiting so an in-flight Delete can still trash
    // its folder file after the rest of the stack is dismissed.
    const requested = artifacts
      .filter((artifact) => !exitingArtifactIds.has(artifact.id))
      .map((artifact) => artifact.id);
    if (requested.length === 0) return;
    replaceClearingArtifactIds(new Set(requested));
    void invoke<string[]>("dismiss_all_artifacts", { artifactIds: requested })
      .catch(() => undefined);
  };

  const setStackCollapsed = (nextCollapsed: boolean) => {
    if (controlsDisabled || stackDrag.current?.isActive || collapsed === nextCollapsed) return;
    if (stackMotionTimer.current) clearTimeout(stackMotionTimer.current);
    const cancelHoverReady = () => {
      if (stackHoverReadyFrames.current) {
        cancelAnimationFrame(stackHoverReadyFrames.current.first);
        cancelAnimationFrame(stackHoverReadyFrames.current.second);
        stackHoverReadyFrames.current = null;
      }
      setStackHoverReady(false);
    };
    // Two frames after collapse so the rest pose is committed before
    // transform easing (hover fan-out) turns back on.
    const armHoverReady = () => {
      cancelHoverReady();
      const frames = { first: 0, second: 0 };
      frames.first = requestAnimationFrame(() => {
        frames.second = requestAnimationFrame(() => {
          stackHoverReadyFrames.current = null;
          setStackHoverReady(true);
        });
      });
      stackHoverReadyFrames.current = frames;
    };
    if (prefersReducedMotion()) {
      setExpandFromTransforms(new Map());
      if (!nextCollapsed) pendingNewestReveal.current = true;
      setStackMotion(nextCollapsed ? "collapsed" : "expanded");
      if (nextCollapsed) armHoverReady();
      else cancelHoverReady();
      void invoke("set_mini_previews_collapsed", { collapsed: nextCollapsed })
        .catch(() => {
          if (!nextCollapsed) pendingNewestReveal.current = false;
          setStackMotion(nextCollapsed ? "expanded" : "collapsed");
          if (nextCollapsed) cancelHoverReady();
          else armHoverReady();
        });
      return;
    }
    if (nextCollapsed) {
      cancelHoverReady();
      setExpandFromTransforms(new Map());
      setStackHoverLatched(false);
      setStackMinimizeRun(false);
      setStackMotion("collapsing");
      const collapsePromise = invoke("set_mini_previews_collapsed", { collapsed: true });
      const frames = { first: 0, second: 0 };
      frames.first = requestAnimationFrame(() => {
        frames.second = requestAnimationFrame(() => {
          stackHoverReadyFrames.current = null;
          setStackMinimizeRun(true);
          stackMotionTimer.current = setTimeout(() => {
            stackMotionTimer.current = null;
            setStackMinimizeRun(false);
            setStackMotion("collapsed");
            setStackHoverLatched(true);
            requestAnimationFrame(() => {
              const target = stackRef.current?.querySelector(
                ".thumbnail-collapsed-hit-target",
              );
              target?.removeAttribute("data-native-pointer-hover");
              if (!target?.matches(":hover")) setStackHoverLatched(false);
            });
            armHoverReady();
          }, STACK_MOTION_MS);
        });
      });
      stackHoverReadyFrames.current = frames;
      void collapsePromise.catch(() => {
        if (stackMotionTimer.current) {
          clearTimeout(stackMotionTimer.current);
          stackMotionTimer.current = null;
        }
        if (stackHoverReadyFrames.current) {
          cancelAnimationFrame(stackHoverReadyFrames.current.first);
          cancelAnimationFrame(stackHoverReadyFrames.current.second);
          stackHoverReadyFrames.current = null;
        }
        cancelHoverReady();
        setStackMinimizeRun(false);
        setStackHoverLatched(false);
        setStackMotion("expanded");
      });
      return;
    }
    pendingNewestReveal.current = true;
    void invoke("set_mini_previews_collapsed", { collapsed: false })
      .then(() => {
        setExpandFromTransforms(captureThumbnailCardTransforms(stackRef.current));
        cancelHoverReady();
        setStackMinimizeRun(false);
        setStackHoverLatched(false);
        setStackMotion("expanding");
        stackMotionTimer.current = setTimeout(() => {
          stackMotionTimer.current = null;
          setExpandFromTransforms(new Map());
          setStackMotion("expanded");
        }, STACK_MOTION_MS);
      })
      .catch(() => {
        setExpandFromTransforms(new Map());
        pendingNewestReveal.current = false;
        setStackMotion("collapsed");
        armHoverReady();
      });
  };

  const scrollStackBy = (slots: number) => {
    const stack = stackRef.current;
    if (!stack) return;
    const cardCount = stack.querySelectorAll(":scope > .thumbnail-card").length;
    const maxScrollTop = Math.max(
      0,
      thumbnailStackContentHeight(cardCount) - stack.clientHeight,
    );
    const targetTop = Math.min(
      maxScrollTop,
      Math.max(0, stack.scrollTop + slots * THUMBNAIL_CARD_SLOT_PX),
    );
    cancelStackScroll.current?.();
    cancelStackScroll.current = animateThumbnailStackScroll(stack, targetTop, {
      reducedMotion: prefersReducedMotion(),
    });
  };

  const collapsedContentHeight = () => thumbnailCollapsedFrameHeight(
    Math.max(
      stackRef.current?.querySelectorAll(":scope > .thumbnail-card").length ?? 1,
      1,
    ),
  );

  const collapsedNativeGeometry = async () => {
    const contentHeight = collapsedContentHeight();
    const scale = currentWindow ? await currentWindow.scaleFactor() : 1;
    const size = currentWindow ? await currentWindow.innerSize() : null;
    const monitor = await currentMonitor();
    const workTop = monitor ? monitor.workArea.position.y / scale : 0;
    const workHeight = monitor
      ? monitor.workArea.size.height / scale
      : window.screen.availHeight;
    return {
      contentHeight,
      frameHeight: thumbnailStackMeasuredFrameHeight(
        size ? size.height / scale : null,
        contentHeight,
        window.innerHeight,
      ),
      work: {
        x: monitor ? monitor.workArea.position.x / scale : 0,
        y: workTop,
        width: monitor
          ? monitor.workArea.size.width / scale
          : window.screen.availWidth,
        height: workHeight,
        bottomGap: 12,
      },
      workTop,
      workHeight,
    };
  };

  const placeCollapsedStackFrame = async (
    x: number,
    y: number,
    anchor: ThumbnailStackAnchor,
    nativeGeometry?: Awaited<ReturnType<typeof collapsedNativeGeometry>>,
  ) => {
    const contentHeight = collapsedContentHeight();
    if (isTauri()) {
      try {
        const { frameHeight, work, workTop, workHeight } = nativeGeometry
          ?? await collapsedNativeGeometry();
        const clamped = clampThumbnailStackFrame(
          x,
          y,
          340,
          frameHeight,
          work,
          contentHeight,
          anchor,
        );
        const next = await invoke<{ x: number; y: number }>(
          "set_mini_preview_stack_position",
          { x: clamped.x, y: clamped.y, anchor },
        );
        applyThumbnailStackGravity(
          stackRef.current,
          thumbnailStackGravityFromWorkArea({
            pileBottom: thumbnailStackVisualPileBottom({
              y: next.y,
              frameHeight,
              contentHeight,
              anchor,
            }),
            workTop,
            workHeight,
            contentHeight,
            bottomGap: 12,
          }),
        );
        commitStackSide(thumbnailStackSideFromBias(
          thumbnailStackBiasFromFrameX(next.x, work.x, work.width),
          stackSideRef.current,
        ));
        return next;
      } catch {
        applyThumbnailStackGravity(
          stackRef.current,
          thumbnailStackGravityFromPlacement(placementRef.current),
        );
        const next = await invoke<{ x: number; y: number }>(
          "set_mini_preview_stack_position",
          { x, y, anchor },
        );
        return next;
      }
    }
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    const written = writeHarnessStackOffset(
      x,
      y,
      document.documentElement,
      viewport,
      { anchor, contentHeight },
    );
    applyThumbnailStackGravity(
      stackRef.current,
      thumbnailStackGravityFromHarness({
        offsetY: written.y,
        anchor,
        viewportHeight: viewport.height,
        contentHeight,
      }),
    );
    commitStackSide(thumbnailStackSideFromBias(
      thumbnailStackBiasFromHarness(written.x, viewport.width),
      stackSideRef.current,
    ));
    return written;
  };

  const moveCollapsedStackFrame = async (x: number, y: number) => {
    const from = collapsedLayoutAnchorRef.current;
    const contentHeight = collapsedContentHeight();
    if (isTauri()) {
      try {
        const nativeGeometry = await collapsedNativeGeometry();
        const { frameHeight, work, workTop, workHeight } = nativeGeometry;
        const sourceFrame = clampThumbnailStackFrame(
          x,
          y,
          340,
          frameHeight,
          work,
          contentHeight,
          from,
        );
        const gravity = thumbnailStackGravityFromWorkArea({
          pileBottom: thumbnailStackVisualPileBottom({
            y: sourceFrame.y,
            frameHeight,
            contentHeight,
            anchor: from,
          }),
          workTop,
          workHeight,
          contentHeight,
          bottomGap: 12,
        });
        const nextAnchor = thumbnailStackAnchorFromGravity(gravity, from);
        if (nextAnchor === from) {
          return placeCollapsedStackFrame(x, y, from, nativeGeometry);
        }
        const converted = convertThumbnailStackFrameAnchor(
          sourceFrame,
          from,
          nextAnchor,
          frameHeight,
          contentHeight,
        );
        collapsedLayoutAnchorRef.current = nextAnchor;
        // The retained window can have hundreds of pixels of empty expanded
        // height. Hold the stack at its converted screen position while the
        // DOM anchor changes and native position IPC catches up; otherwise
        // the new alignment can paint at the opposite edge for one frame.
        const stack = stackRef.current;
        if (stack) stack.style.translate = `0 ${converted.y - sourceFrame.y}px`;
        commitStackAnchor(nextAnchor);
        try {
          const next = await placeCollapsedStackFrame(
            converted.x,
            converted.y,
            nextAnchor,
            nativeGeometry,
          );
          stackDrag.current?.rebaseFrame(next, sourceFrame);
          return next;
        } finally {
          stack?.style.removeProperty("translate");
        }
      } catch {
        collapsedLayoutAnchorRef.current = from;
        commitStackAnchor(from);
        return placeCollapsedStackFrame(x, y, from);
      }
    }

    const viewport = { width: window.innerWidth, height: window.innerHeight };
    const gravity = thumbnailStackGravityFromHarness({
      offsetY: y,
      anchor: from,
      viewportHeight: viewport.height,
      contentHeight,
    });
    const nextAnchor = thumbnailStackAnchorFromGravity(gravity, from);
    if (nextAnchor === from) return placeCollapsedStackFrame(x, y, from);
    const converted = convertHarnessStackOffsetAnchor(
      { x, y },
      from,
      nextAnchor,
      viewport.height,
      contentHeight,
    );
    collapsedLayoutAnchorRef.current = nextAnchor;
    const stack = stackRef.current;
    if (stack) stack.style.translate = `0 ${converted.y - y}px`;
    commitStackAnchor(nextAnchor);
    try {
      const next = await placeCollapsedStackFrame(converted.x, converted.y, nextAnchor);
      stackDrag.current?.rebaseFrame(next, { x, y });
      return next;
    } finally {
      stack?.style.removeProperty("translate");
    }
  };

  const collapsedStackDrag = () => {
    stackDrag.current ??= new CollapsedThumbnailStackDrag({
      getFrame: async () => {
        if (currentWindow) {
          const scale = await currentWindow.scaleFactor();
          const position = await currentWindow.outerPosition();
          return { x: position.x / scale, y: position.y / scale };
        }
        return readHarnessStackOffset();
      },
      moveFrame: moveCollapsedStackFrame,
      reducedMotion: prefersReducedMotion,
      onSway: (sway) => applyThumbnailStackDragSway(stackRef.current, sway),
      onDraggingChange: (dragging) => {
        const stack = stackRef.current;
        if (!stack) return;
        setThumbnailStackDragging(stack, dragging);
        if (dragging && stackFanCollapsed.current) {
          setThumbnailStackDragSwayReady(stack, true);
        }
        window.dispatchEvent(new Event(THUMBNAIL_HIT_TEST_CHANGED_EVENT));
      },
    });
    return stackDrag.current;
  };

  const onCollapsedStackPointerDown = (event: React.PointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0 || controlsDisabled) return;
    if (stackDrag.current?.isActive) {
      // A press during the previous drop must not become an Expand click
      // after that drop's asynchronous anchor conversion finishes.
      skipCollapsedStackClick.current = true;
      event.preventDefault();
      return;
    }
    collapsedStackPointerCleanup.current?.();
    const drag = collapsedStackDrag();
    if (!drag.pointerDown(event.nativeEvent)) return;
    collapsedLayoutAnchorRef.current = stackAnchorRef.current;
    skipCollapsedStackClick.current = true;
    setThumbnailStackPressing(stackRef.current, true);
    if (stackFanCollapseTimer.current) {
      clearTimeout(stackFanCollapseTimer.current);
      stackFanCollapseTimer.current = null;
    }
    if (prefersReducedMotion()) {
      stackFanCollapsed.current = true;
    } else {
      stackFanCollapsed.current = false;
      const cardCount = stackRef.current?.querySelectorAll(":scope > .thumbnail-card").length
        ?? 0;
      stackFanCollapseTimer.current = setTimeout(() => {
        stackFanCollapseTimer.current = null;
        stackFanCollapsed.current = true;
        if (!drag.isDragging) return;
        drag.resetSway();
        setThumbnailStackDragSwayReady(stackRef.current, true);
      }, thumbnailStackFanCollapseMs(cardCount));
    }
    event.preventDefault();
    event.nativeEvent.preventDefault();
    const hitTarget = event.currentTarget;
    const pointerId = event.pointerId;
    retainThumbnailPointerCapture(hitTarget, pointerId);
    let finished = false;
    let recapturing = false;
    const onMove = (moveEvent: PointerEvent) => {
      if (moveEvent.pointerId !== pointerId) return;
      moveEvent.preventDefault();
      retainThumbnailPointerCapture(hitTarget, pointerId);
      void drag.pointerMove(moveEvent).catch(() => undefined);
    };
    const finishPointer = (
      upEvent: Pick<PointerEvent, "pointerId" | "clientX" | "clientY">,
      options: { expand?: boolean } = {},
    ) => {
      if (finished || upEvent.pointerId !== pointerId) return;
      finished = true;
      collapsedStackPointerCleanup.current = null;
      window.removeEventListener("pointermove", onMove, true);
      window.removeEventListener("pointerup", onPointerUp, true);
      window.removeEventListener("pointercancel", onPointerUp, true);
      window.removeEventListener("mouseup", onMouseUp, true);
      hitTarget.removeEventListener("lostpointercapture", onLostCapture);
      if (stackFanCollapseTimer.current) {
        clearTimeout(stackFanCollapseTimer.current);
        stackFanCollapseTimer.current = null;
      }
      stackFanCollapsed.current = false;
      setThumbnailStackPressing(stackRef.current, false);
      releaseThumbnailPointerCapture(hitTarget, pointerId);
      releaseThumbnailCapturedHover(hitTarget, {
        x: upEvent.clientX,
        y: upEvent.clientY,
      });
      void drag.pointerUp({ pointerId: upEvent.pointerId }).catch(() => "ignored" as const)
        .then((outcome) => {
          setThumbnailStackDragging(stackRef.current, false);
          window.dispatchEvent(new Event(THUMBNAIL_HIT_TEST_CHANGED_EVENT));
          if (options.expand !== false && outcome === "expand") setStackCollapsed(false);
        });
    };
    const onPointerUp = (upEvent: PointerEvent) => finishPointer(upEvent);
    const onMouseUp = (upEvent: MouseEvent) => {
      if (upEvent.button !== 0) return;
      finishPointer({
        pointerId,
        clientX: upEvent.clientX,
        clientY: upEvent.clientY,
      });
    };
    const onLostCapture = (lostEvent: PointerEvent) => {
      if (finished || recapturing || lostEvent.pointerId !== pointerId) return;
      recapturing = true;
      const recaptured = retainThumbnailPointerCapture(hitTarget, pointerId);
      recapturing = false;
      if (thumbnailLostPointerCaptureShouldEndDrag(lostEvent, recaptured)) {
        finishPointer(lostEvent);
      }
    };
    collapsedStackPointerCleanup.current = () => finishPointer(
      { pointerId, clientX: -1, clientY: -1 },
      { expand: false },
    );
    window.addEventListener("pointermove", onMove, { capture: true, passive: false });
    window.addEventListener("pointerup", onPointerUp, true);
    window.addEventListener("pointercancel", onPointerUp, true);
    window.addEventListener("mouseup", onMouseUp, true);
    hitTarget.addEventListener("lostpointercapture", onLostCapture);
  };

  return (
    <>
      <main
        ref={stackRef}
        className={[
          "thumbnail-stack",
          compact ? "thumbnail-stack-compact" : "",
          stackScrollport ? THUMBNAIL_STACK_SCROLLPORT_CLASS : "",
          stackMotion === "collapsing" ? "thumbnail-stack-minimizing" : "",
          stackMotion === "collapsed" ? "thumbnail-stack-minimized" : "",
          stackMotion === "expanding" ? "thumbnail-stack-expanding" : "",
          stackMinimizeRun ? "thumbnail-stack-minimize-run" : "",
          stackHoverReady ? "thumbnail-stack-hover-ready" : "",
          stackHoverLatched ? "thumbnail-stack-hover-latched" : "",
          stackAnchor === "top" ? "thumbnail-stack-anchor-top" : "",
          stackClearing ? "thumbnail-stack-clearing" : "",
        ].filter(Boolean).join(" ")}
        onScroll={refreshStackOverflow}
        onDragStartCapture={(event) => {
          if (compact) preventThumbnailHtml5Drag(event.nativeEvent);
        }}
      >
        {/* Horizontal-only Gaussian blur for dismiss motion streak (stdDeviation x 0). */}
        <svg className="thumbnail-svg-defs" aria-hidden="true" focusable="false">
          <defs>
            <filter id="thumbnail-motion-blur-a" x="-50%" y="-20%" width="200%" height="140%" colorInterpolationFilters="sRGB">
              <feGaussianBlur stdDeviation="3.5 0" />
            </filter>
            <filter id="thumbnail-motion-blur-b" x="-60%" y="-20%" width="220%" height="140%" colorInterpolationFilters="sRGB">
              <feGaussianBlur stdDeviation="8 0" />
            </filter>
            <filter id="thumbnail-motion-blur-c" x="-70%" y="-20%" width="240%" height="140%" colorInterpolationFilters="sRGB">
              <feGaussianBlur stdDeviation="14 0" />
            </filter>
          </defs>
        </svg>
        {(stackAnchor === "top" && stackMotion !== "collapsed"
          ? [...artifacts].reverse()
          : artifacts
        ).map((artifact, _visualIndex, visualArtifacts) => {
          const isStackClearTarget = clearingArtifactIds.has(artifact.id);
          const clearingOrder = visualArtifacts
            .map((item) => item.id)
            .filter((id) => clearingArtifactIds.has(id));
          const clearingIndex = clearingOrder.indexOf(artifact.id);
          const clearDelayMs = isStackClearTarget && !prefersReducedMotion()
            ? Math.min(
              (clearingOrder.length - 1 - clearingIndex) * THUMBNAIL_CLEAR_STAGGER_MS,
              THUMBNAIL_CLEAR_STAGGER_MAX_MS,
            )
            : 0;
          return (
          <ThumbnailCard
            key={artifact.id}
            artifact={artifact}
            clipboardCurrent={clipboardState.artifact_id === artifact.id}
            viewerActive={activeViewerArtifactId === artifact.id}
            editorActive={editorActiveArtifactIds.has(artifact.id)}
            stackCollapsed={compact}
            stackDepth={artifacts.length - artifacts.indexOf(artifact) - 1}
            expandFromTransform={expandFromTransforms.get(artifact.id)}
            stackDismissing={isStackClearTarget}
            clearDelayMs={clearDelayMs}
            previewDropReject={
              previewRejectShake
              && artifacts.length - artifacts.indexOf(artifact) - 1 === 0
            }
            onRemoved={(artifactId) => {
              setArtifactExiting(artifactId, false);
              const current = clearingArtifactIdsRef.current;
              if (current.has(artifactId)) {
                const next = new Set(current);
                next.delete(artifactId);
                replaceClearingArtifactIds(next);
              }
              setArtifacts((current) => current.filter(({ id }) => id !== artifactId));
            }}
            onExitChange={setArtifactExiting}
          />
          );
        })}
        {collapsed && (
          <button
            type="button"
            className="thumbnail-collapsed-hit-target"
            aria-label={`Expand ${artifacts.length === 1 ? "preview" : `${artifacts.length} previews`}`}
            draggable={false}
            disabled={controlsDisabled}
            style={{
              "--thumbnail-collapsed-peek": `${thumbnailCollapsedPeekPx(artifacts.length)}px`,
              "--thumbnail-collapsed-hover-peek": `${thumbnailCollapsedPeekPx(artifacts.length, true)}px`,
            } as CSSProperties}
            onPointerDown={onCollapsedStackPointerDown}
            onDragStart={(event) => preventThumbnailHtml5Drag(event.nativeEvent)}
            onPointerEnter={(event) => {
              armThumbnailCollapsedHover(event.currentTarget);
            }}
            onPointerLeave={(event) => {
              event.currentTarget.removeAttribute("data-native-pointer-hover");
              setThumbnailCollapsedHoverStale(event.currentTarget, true);
              setStackHoverLatched(false);
            }}
            onClick={() => {
              if (skipCollapsedStackClick.current) {
                skipCollapsedStackClick.current = false;
                return;
              }
              setStackCollapsed(false);
            }}
          />
        )}
      </main>
      {!collapsed && (
        <div className={[
          "thumbnail-stack-toolbar",
          stackAnchor === "top" ? "thumbnail-stack-toolbar-anchor-top" : "",
          stackSide === "right" ? "thumbnail-stack-toolbar-anchor-right" : "",
          stackMotion === "collapsing" ? "thumbnail-stack-toolbar-leaving" : "",
          stackMotion === "expanding" ? "thumbnail-stack-toolbar-entering" : "",
          (stackClearing || exitingOnly) && stackMotion !== "collapsing"
            ? stackClearing
              ? "thumbnail-stack-toolbar-clearing"
              : "thumbnail-stack-toolbar-exiting"
            : "",
        ].filter(Boolean).join(" ")}>
          {showClearAll && (
            <button
              type="button"
              className="thumbnail-stack-control thumbnail-stack-clear"
              aria-label="Clear all previews"
              data-tooltip="Clear all"
              disabled={controlsDisabled}
              onClick={clearAllPreviews}
            >
              <CloseIcon />
            </button>
          )}
          <button
            type="button"
            className="thumbnail-stack-control thumbnail-stack-minimize"
            aria-label="Minimize previews"
            onClick={() => setStackCollapsed(true)}
          >
            <PreviewStackIcon />
            <span className="thumbnail-stack-minimize-label" aria-hidden="true">
              Show less
            </span>
          </button>
        </div>
      )}
      {showOverflowCues && stackOverflow.hasOlder && (
        <button
          type="button"
          className="thumbnail-overflow-cue thumbnail-overflow-cue-older"
          aria-label={stackAnchor === "top" ? "Show newer captures" : "Show older captures"}
          onClick={() => scrollStackBy(-1)}
        >
          <ThumbnailOverflowChevron direction="up" />
        </button>
      )}
      {showOverflowCues && stackOverflow.hasNewer && (
        <button
          type="button"
          className="thumbnail-overflow-cue thumbnail-overflow-cue-newer"
          aria-label={stackAnchor === "top" ? "Show older captures" : "Show newer captures"}
          onClick={() => scrollStackBy(1)}
        >
          <ThumbnailOverflowChevron direction="down" />
        </button>
      )}
    </>
  );
}

function ThumbnailOverflowChevron({ direction }: { direction: "up" | "down" }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d={direction === "up" ? "M3.5 10 8 5.5 12.5 10" : "M3.5 6 8 10.5 12.5 6"} />
    </svg>
  );
}

function PreviewStackIcon() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="m3 5.5 5-2.75 5 2.75-5 2.75L3 5.5Z" />
      <path d="m3.5 8.5 4.5 2.5 4.5-2.5" />
      <path d="m4.5 11 3.5 2 3.5-2" />
    </svg>
  );
}

export function ThumbnailCard({
  artifact,
  clipboardCurrent,
  viewerActive,
  editorActive = false,
  stackCollapsed = false,
  stackDepth = 0,
  expandFromTransform,
  stackDismissing = false,
  clearDelayMs = 0,
  previewDropReject = false,
  onRemoved,
  onExitChange,
}: {
  artifact: CaptureArtifact;
  clipboardCurrent: boolean;
  viewerActive: boolean;
  /** True when this capture is still present as a layer in an open editor. */
  editorActive?: boolean;
  stackCollapsed?: boolean;
  stackDepth?: number;
  expandFromTransform?: string;
  /** Parent Clear all: play Close without a per-card dismiss command. */
  stackDismissing?: boolean;
  clearDelayMs?: number;
  /** Dev harness: loop the self-drop “no” shake on this card. */
  previewDropReject?: boolean;
  onRemoved: (artifactId: string) => void;
  onExitChange?: (artifactId: string, exiting: boolean) => void;
}) {
  const [feedback, setFeedback] = useState<"saved" | null>(null);
  const [busy, setBusy] = useState<"copied" | "saved" | null>(null);
  const [error, setError] = useState("");
  const [thumbnailReady, setThumbnailReady] = useState(false);
  const [arrived, setArrived] = useState(false);
  const [fileDragging, setFileDragging] = useState(false);
  const [dropRejected, setDropRejected] = useState(false);
  const [exit, setExit] = useState<"dismiss" | "delete" | null>(null);
  const [dustParticles, setDustParticles] = useState<ThumbnailDustParticle[] | null>(null);
  /** Optimistically morph Edit into In editor while the native window opens. */
  const [editorOpening, setEditorOpening] = useState(false);
  /**
   * Editor control labels + card ring stay in the leave path so width/ring can
   * ease when the editor closes. After the morph, `lingering` keeps the plain
   * Edit icon visible briefly so a mis-close can be reopened without hover.
   * `trackedActive` mirrors the last `editorActive` prop we adjusted for, so we
   * can update presence during render (avoids setState-in-effect).
   */
  const [editorPresence, setEditorPresence] = useState({
    visible: editorActive,
    leaving: false,
    lingering: false,
    trackedActive: editorActive,
  });
  if (editorActive !== editorPresence.trackedActive) {
    if (editorActive) {
      setEditorPresence({
        visible: true,
        leaving: false,
        lingering: false,
        trackedActive: true,
      });
    } else {
      setEditorPresence({
        visible: editorPresence.visible,
        leaving: editorPresence.visible,
        lingering: false,
        trackedActive: false,
      });
    }
  }
  if (editorActive && editorOpening) {
    setEditorOpening(false);
  }
  const editorPresenceVisible = editorPresence.visible;
  const editorPresenceLeaving = editorPresence.leaving;
  const editorPresenceLingering = editorPresence.lingering;
  /**
   * Snapshot of chrome labels taken the moment exit starts.
   * While `isExiting`, UI is frozen on this snapshot — no “Saved to Folder!”→
   * “Show in Folder” flips, clipboard badge changes, or other prop-driven transitions.
   */
  const [exitChrome, setExitChrome] = useState<{
    feedback: "saved" | null;
    hasPath: boolean;
    clipboardCurrent: boolean;
    historySaved: boolean;
    copyFailed: boolean;
    editorActive: boolean;
  } | null>(null);
  const cardRef = useRef<HTMLElement>(null);
  const dustLayerRef = useRef<HTMLDivElement>(null);
  const fileDraggingRef = useRef(false);
  const exitAction = useRef<string | null>(null);
  /**
   * Exit lock: once true, this card is frozen for the whole dismiss/delete
   * animation. Blocks clicks, async action completions, timers, and any new
   * chrome transitions. Prefer this over ad-hoc checks when adding features.
   */
  const exitingRef = useRef(false);
  const feedbackTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const exitFallbackTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const dropRejectTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  /** Synchronous lock check for async/timer paths (state may lag a frame). */
  const isExitLocked = () => exitingRef.current;
  const isExiting = exit !== null;

  const markThumbnailReady = () => {
    void invoke("thumbnail_ready", { artifactId: artifact.id })
      .catch(() => undefined)
      .finally(() => {
        // Start the arrival glow only after the native thumbnail window is
        // ready to show, so none of its 2.5 seconds elapse while hidden.
        setThumbnailReady(true);
        window.dispatchEvent(new Event("captures-thumbnail-ready"));
      });
  };

  useEffect(() => {
    return () => {
      if (feedbackTimer.current) clearTimeout(feedbackTimer.current);
      if (exitFallbackTimer.current) clearTimeout(exitFallbackTimer.current);
      if (dropRejectTimer.current) clearTimeout(dropRejectTimer.current);
    };
  }, []);

  useLayoutEffect(() => {
    restoreThumbnailStackShiftClass(cardRef.current);
  });

  useEffect(() => {
    if (!previewDropReject || !arrived || stackCollapsed) return;
    const kick = () => {
      setDropRejected(false);
      requestAnimationFrame(() => {
        if (exitingRef.current) return;
        setDropRejected(true);
      });
    };
    const start = window.setTimeout(kick, 400);
    const loop = window.setInterval(kick, 1_000);
    return () => {
      window.clearTimeout(start);
      window.clearInterval(loop);
    };
  }, [previewDropReject, arrived, stackCollapsed]);

  // After presence leaves, drop leave-held labels/ring once the ease finishes,
  // then hold the plain Edit icon for a short recovery window.
  useEffect(() => {
    if (!editorPresenceLeaving) return;
    const leaveMs = prefersReducedMotion() ? 0 : EDITOR_PRESENCE_LEAVE_MS;
    const timer = window.setTimeout(() => {
      setEditorPresence((current) => (
        current.leaving
          ? {
            visible: false,
            leaving: false,
            lingering: true,
            trackedActive: current.trackedActive,
          }
          : current
      ));
    }, leaveMs);
    return () => window.clearTimeout(timer);
  }, [editorPresenceLeaving]);

  useEffect(() => {
    if (!editorPresenceLingering) return;
    const timer = window.setTimeout(() => {
      setEditorPresence((current) => (
        current.lingering
          ? { ...current, lingering: false }
          : current
      ));
    }, EDITOR_PRESENCE_LINGER_MS);
    return () => window.clearTimeout(timer);
  }, [editorPresenceLingering]);

  // WAAPI chip flight — avoids CSS custom-property keyframes that WebView2 drops.
  // Depend on `exit` too so the layer is mounted before we query chips.
  useEffect(() => {
    if (exit !== "delete" || !dustParticles || dustParticles.length === 0) return;
    const layer = dustLayerRef.current;
    if (!layer) return;
    const chips = layer.querySelectorAll(".thumbnail-dust");
    return playThumbnailDustAnimations(chips, dustParticles);
  }, [dustParticles, exit]);

  const showSavedFeedback = () => {
    if (isExitLocked()) return;
    setFeedback("saved");
    if (feedbackTimer.current) clearTimeout(feedbackTimer.current);
    feedbackTimer.current = setTimeout(() => {
      if (isExitLocked()) return;
      setFeedback(null);
    }, THUMBNAIL_SAVED_FEEDBACK_MS);
  };

  const runAction = async (
    action: string,
    success?: "copied" | "saved",
  ): Promise<boolean> => {
    if (isExitLocked() || isExiting) return false;
    if (success && busy) return false;
    setError("");
    if (success) setBusy(success);
    try {
      await invoke(action, { artifactId: artifact.id });
      if (isExitLocked()) return false;
      if (success === "saved") showSavedFeedback();
      return true;
    } catch (error) {
      if (!isExitLocked()) setError(String(error));
      return false;
    } finally {
      if (success && !isExitLocked()) setBusy(null);
    }
  };

  const openEditor = (event: React.MouseEvent<HTMLButtonElement>) => {
    if (isExitLocked() || isExiting) return;
    const control = event.currentTarget;
    markThumbnailEditorControlOpened(control);
    setEditorOpening(true);
    if (cardRef.current) setThumbnailNativeActiveCard(cardRef.current);
    void runAction("open_screenshot_editor").then((opened) => {
      if (opened || isExitLocked()) return;
      setEditorOpening(false);
      rearmThumbnailEditorControlHover(control);
    });
  };

  const playDropReject = () => {
    setDropRejected(false);
    if (dropRejectTimer.current) {
      clearTimeout(dropRejectTimer.current);
      dropRejectTimer.current = null;
    }
    requestAnimationFrame(() => {
      if (isExitLocked() || isExiting) return;
      setDropRejected(true);
      // animationend can be skipped in a hidden webview; don’t leave the class on.
      dropRejectTimer.current = setTimeout(() => {
        dropRejectTimer.current = null;
        setDropRejected(false);
      }, THUMBNAIL_DROP_REJECT_MS);
    });
  };

  const finishFileDrag = (
    result: "Dropped" | "Cancelled",
    cursorPos: { x: number; y: number },
  ) => {
    fileDraggingRef.current = false;
    setFileDragging(false);
    // Native OS file drags can leave the always-on-top preview stack
    // click-through or without hover tracking. Always re-arm after the drag ends.
    window.dispatchEvent(new Event(THUMBNAIL_HIT_TEST_CHANGED_EVENT));
    void invoke("refresh_thumbnail_interactivity").catch(() => undefined);

    if (isExitLocked() || isExiting) return;

    void (async () => {
      let landing: PreviewFileDropLanding = result === "Dropped" ? "external" : "app_window";
      try {
        const reported = await invoke<PreviewFileDropLanding>("preview_file_drop_landing", {
          x: Number(cursorPos.x),
          y: Number(cursorPos.y),
        });
        if (isPreviewFileDropLanding(reported)) landing = reported;
      } catch {
        // Keep the Dropped → dismiss fallback when the native hit test fails.
      }
      if (isExitLocked() || isExiting) return;
      // Dropping the file back on this stack captures the preview itself
      // (hall of mirrors) and used to dismiss the card. Refuse it with a shake.
      if (previewFileDropShouldReject(landing)) {
        playDropReject();
        return;
      }
      // Drops into Captures itself (screenshot editor, other app windows) keep
      // the preview. External targets (Finder, Slack, browser) still dismiss.
      if (!previewFileDropShouldDismiss(result, landing)) return;
      exitWith("dismiss", "dismiss_artifact");
    })();
  };

  const beginFileDrag = async (event: React.DragEvent<HTMLImageElement>) => {
    event.preventDefault();
    if (fileDraggingRef.current || isExitLocked() || isExiting) {
      return;
    }
    fileDraggingRef.current = true;
    setFileDragging(true);
    setError("");
    try {
      const payload = await invoke<ArtifactDragPayload>("prepare_artifact_drag", {
        artifactId: artifact.id,
      });
      await startDrag(
        {
          item: [payload.path],
          icon: payload.icon_path,
          mode: "copy",
        },
        ({ result, cursorPos }) => {
          finishFileDrag(result, {
            x: Number(cursorPos.x),
            y: Number(cursorPos.y),
          });
        },
      );
    } catch (error) {
      fileDraggingRef.current = false;
      setFileDragging(false);
      window.dispatchEvent(new Event(THUMBNAIL_HIT_TEST_CHANGED_EVENT));
      void invoke("refresh_thumbnail_interactivity").catch(() => undefined);
      setError(String(error));
    }
  };

  const completeExit = () => {
    const action = exitAction.current;
    if (!action) return;
    exitAction.current = null;
    if (exitFallbackTimer.current) {
      clearTimeout(exitFallbackTimer.current);
      exitFallbackTimer.current = null;
    }
    // A second exit can retarget the survivor transform while this card's own
    // animation is finishing. Releasing the held slot at that instant makes
    // the flex reflow race the in-flight compositor transition and visibly
    // snap. Wait for the browser's live stack transition, not a fixed estimate.
    void waitForThumbnailStackSettle(cardRef.current)
      .then(() => {
        if (action === STACK_CLEAR_EXIT_ACTION) return;
        return invoke(action, { artifactId: artifact.id });
      })
      .then(() => {
        onRemoved(artifact.id);
        // The outgoing inert card can make the native thumbnail window
        // click-through while an editor owns focus. Re-arm immediately after
        // removal instead of waiting for a throttled background pointer poll.
        window.dispatchEvent(new Event(THUMBNAIL_HIT_TEST_CHANGED_EVENT));
        if (action !== STACK_CLEAR_EXIT_ACTION) {
          void invoke("refresh_thumbnail_interactivity").catch(() => undefined);
        }
      })
      .catch((error) => {
        // Only unlock if remove failed — otherwise the card is gone.
        exitingRef.current = false;
        onExitChange?.(artifact.id, false);
        setExit(null);
        setExitChrome(null);
        setDustParticles(null);
        setError(String(error));
        window.dispatchEvent(new Event(THUMBNAIL_HIT_TEST_CHANGED_EVENT));
        void invoke("refresh_thumbnail_interactivity").catch(() => undefined);
      });
  };

  const exitWith = (kind: "dismiss" | "delete", action: string) => {
    if (isExitLocked() || isExiting) return;
    // Acquire the exit lock first so any in-flight async work becomes a no-op.
    exitingRef.current = true;
    exitAction.current = action;
    onExitChange?.(artifact.id, true);
    if (feedbackTimer.current) {
      clearTimeout(feedbackTimer.current);
      feedbackTimer.current = null;
    }
    // Freeze chrome *as rendered now* — never flip “Saved to Folder!” into Show in Folder.
    // Keep the in-editor control expanded if presence is still easing out.
    setExitChrome({
      feedback,
      hasPath: Boolean(artifact.path),
      clipboardCurrent,
      historySaved: artifact.history_saved,
      copyFailed: artifact.clipboard_copy_status === "failed",
      editorActive: editorActive || editorOpening || editorPresenceVisible,
    });
    setBusy(null);
    setError("");
    // Build dust in the same turn as setExit so the first painted frame uses
    // the dissolve animation (not the scale/blur fallback).
    if (kind === "delete" && !prefersReducedMotion()) {
      const card = cardRef.current;
      const width = card?.clientWidth || THUMBNAIL_CARD_FALLBACK_WIDTH;
      const height = card?.clientHeight || THUMBNAIL_CARD_FALLBACK_HEIGHT;
      // Before a folder save the delete control is the first top-left button;
      // after save it sits next to Close — wave origin must match the real icon.
      const hasFolderFile = Boolean(artifact.path);
      setDustParticles(buildThumbnailDustParticles(width, height, {
        imageWidth: artifact.width,
        imageHeight: artifact.height,
        originX: hasFolderFile ? THUMBNAIL_DELETE_ORIGIN_AFTER_CLOSE_X : THUMBNAIL_DELETE_ORIGIN_FIRST_X,
        originY: THUMBNAIL_DELETE_ORIGIN_Y,
      }));
    } else {
      setDustParticles(null);
    }
    setExit(kind);
    // Re-run the native hit test after React marks this card non-interactive.
    // Only the outgoing slot should pass clicks through; sibling cards remain usable.
    window.dispatchEvent(new Event(THUMBNAIL_HIT_TEST_CHANGED_EVENT));
    // WebView animation events can be skipped when Windows hides or occludes
    // this always-on-top window. Never leave a deleted card and its backend
    // artifact waiting forever for animationend.
    exitFallbackTimer.current = setTimeout(
      completeExit,
      (kind === "delete" ? THUMBNAIL_DELETE_FALLBACK_MS : THUMBNAIL_DISMISS_FALLBACK_MS)
        + (kind === "dismiss" ? clearDelayMs : 0),
    );
  };

  useLayoutEffect(() => {
    if (!stackDismissing) return;
    exitWith("dismiss", STACK_CLEAR_EXIT_ACTION);
    // One-shot when Clear all marks this live card. exitWith is a render closure.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stackDismissing]);

  const finishExit = (event: React.AnimationEvent<HTMLElement>) => {
    // Ignore bubbled animationend from image streak / dust chips / chrome wave / clip layer.
    if (!exit || event.target !== event.currentTarget || !exitAction.current) return;
    const expectedNames = exit === "delete"
      ? dustParticles && dustParticles.length > 0
        ? ["thumbnail-delete"]
        : ["thumbnail-delete-fallback"]
      : ["thumbnail-dismiss"];
    if (!expectedNames.includes(event.animationName)) return;
    completeExit();
  };
  // While exiting, always render the frozen chrome snapshot.
  const chrome = exitChrome ?? {
    feedback,
    hasPath: Boolean(artifact.path),
    clipboardCurrent,
    historySaved: artifact.history_saved,
    copyFailed: artifact.clipboard_copy_status === "failed",
    editorActive: editorActive || editorOpening || editorPresenceVisible,
  };
  // One top-right control: compact Edit ↔ present “In editor” pill.
  // Live `editorActive` drives the expanded style; leave keeps labels mounted
  // briefly so width can ease back, and the card ring uses the leaving class.
  const editorControlPresent = isExiting ? chrome.editorActive : editorActive || editorOpening;
  const editorControlLeaving = !isExiting && editorPresenceLeaving;
  const editorControlLingering = !isExiting && editorPresenceLingering;
  const mountEditorLabels = isExiting
    ? chrome.editorActive
    : editorOpening || editorPresenceVisible;
  const editorControlAriaLabel = editorControlPresent ? "Show in editor" : "Edit";
  // Before a folder save: trash discards the preview (dissolve). After: trash deletes the file.
  // Close only appears once a file exists so you can hide the preview without trashing it.
  const usingDust = exit === "delete" && dustParticles !== null && dustParticles.length > 0;

  return (
    <article
      ref={cardRef}
      className={[
        "thumbnail-card",
        thumbnailReady ? "thumbnail-ready" : "thumbnail-pending",
        arrived ? "thumbnail-arrived" : "",
        thumbnailReady ? "thumbnail-capture-highlight" : "",
        viewerActive && (!isExiting || exit === "delete") ? "thumbnail-viewer-active" : "",
        editorControlPresent && (!isExiting || exit === "delete") ? "thumbnail-editor-active" : "",
        editorControlLeaving ? "thumbnail-editor-leaving" : "",
        editorControlLingering ? "thumbnail-editor-lingering" : "",
        fileDragging ? "thumbnail-file-dragging" : "",
        dropRejected ? "thumbnail-drop-rejected" : "",
        exit ? `thumbnail-exit-${exit}` : "",
        usingDust ? "thumbnail-exit-dust" : "",
        isExiting ? "thumbnail-exiting" : "",
      ].filter(Boolean).join(" ")}
      data-thumbnail-id={artifact.id}
      style={stackCollapsed || clearDelayMs > 0 ? {
        ...(stackCollapsed ? {
          "--thumbnail-stack-base-depth": stackDepth,
          "--thumbnail-stack-peek-jitter": `${thumbnailStackPeekJitterPx(stackDepth)}px`,
          ...(expandFromTransform
            ? { "--thumbnail-stack-expand-from": expandFromTransform }
            : {}),
        } : {}),
        ...(clearDelayMs > 0
          ? { "--thumbnail-clear-delay": `${clearDelayMs}ms` }
          : {}),
      } as CSSProperties : undefined}
      // HTML inert disables all descendant input/focus while the card is decorative.
      inert={isExiting || stackCollapsed ? true : undefined}
      aria-hidden={stackCollapsed || undefined}
      aria-busy={isExiting || fileDragging}
      data-exit-locked={isExiting ? "true" : undefined}
      data-file-dragging={fileDragging ? "true" : undefined}
      onAnimationEnd={(event) => {
        finishExit(event);
        if (event.target !== event.currentTarget) return;
        if (event.animationName === "thumbnail-arrive") {
          setArrived(true);
        }
        if (event.animationName === THUMBNAIL_DROP_REJECT_ANIMATION) {
          if (dropRejectTimer.current) {
            clearTimeout(dropRejectTimer.current);
            dropRejectTimer.current = null;
          }
          setDropRejected(false);
        }
      }}
    >
      {/* Media shell clips hover blur/scale so it never bleeds into the card ring.
          Dust stays outside this shell so dissolve chips can fly past the edge. */}
      <div
        className="thumbnail-media"
        style={stackCollapsed ? { backgroundImage: cssUrl(artifact.full_url) } : undefined}
      >
        <img
          className={usingDust ? "thumbnail-dust-source" : undefined}
          src={artifact.full_url}
          alt="Screenshot preview"
          hidden={stackCollapsed}
          draggable={!isExiting && !stackCollapsed}
          onDragStart={(event) => {
            if (stackCollapsed) {
              preventThumbnailHtml5Drag(event.nativeEvent);
              return;
            }
            void beginFileDrag(event);
          }}
          onLoad={markThumbnailReady}
          onError={markThumbnailReady}
        />
      </div>
      {usingDust && (
        <div ref={dustLayerRef} className="thumbnail-dust-layer" aria-hidden="true">
          {dustParticles.map((particle) => (
            <span
              key={particle.id}
              className="thumbnail-dust"
              style={{
                left: particle.left,
                top: particle.top,
                width: particle.width,
                height: particle.height,
              }}
            >
              <span
                className="thumbnail-dust-surface"
                style={{
                  left: -particle.sourceLeft,
                  top: -particle.sourceTop,
                  width: particle.cardWidth,
                  height: particle.cardHeight,
                  backgroundImage: `url(${JSON.stringify(artifact.preview_url).slice(1, -1)})`,
                  backgroundSize: `${particle.surfaceWidth}px ${particle.surfaceHeight}px`,
                  backgroundPosition: `${particle.surfaceOffsetX}px ${particle.surfaceOffsetY}px`,
                }}
              />
            </span>
          ))}
        </div>
      )}
      <div className="thumbnail-top-actions">
        <div className="thumbnail-top-left">
          {chrome.hasPath ? (
            <>
              <IconButton
                className="close"
                label="Close"
                disabled={isExiting}
                onClick={() => exitWith("dismiss", "dismiss_artifact")}
              >
                <CloseIcon />
              </IconButton>
              <IconButton
                className="delete"
                label="Delete"
                disabled={isExiting}
                onClick={() => exitWith("delete", "trash_artifact")}
              >
                <TrashIcon />
              </IconButton>
            </>
          ) : (
            <IconButton
              className="delete"
              label="Delete"
              disabled={isExiting}
              onClick={() => exitWith("delete", "dismiss_artifact")}
            >
              <TrashIcon />
            </IconButton>
          )}
        </div>
      </div>
      <button
        type="button"
        className={[
          "thumbnail-editor-control",
          editorControlPresent ? "is-present" : "",
          editorControlLeaving ? "leaving" : "",
        ].filter(Boolean).join(" ")}
        aria-label={editorControlAriaLabel}
        aria-pressed={editorControlPresent || undefined}
        disabled={isExiting}
        onClick={isExiting ? undefined : openEditor}
        onPointerLeave={(event) => {
          rearmThumbnailEditorControlHover(event.currentTarget, { fromLeave: true });
        }}
      >
        <span className="thumbnail-editor-control-face">
          <EditIcon />
          {mountEditorLabels && (
            <span className="thumbnail-editor-control-label" aria-hidden="true">
              <span className="label-rest">In editor</span>
              <span className="label-hover">Show in editor</span>
            </span>
          )}
        </span>
        {!(editorControlPresent || editorControlLeaving) && (
          <span className="thumbnail-editor-control-tip" aria-hidden="true">Edit</span>
        )}
      </button>
      <div className="thumbnail-main-actions">
        {!chrome.clipboardCurrent && (
          <button
            type="button"
            disabled={busy !== null || isExiting}
            onClick={() => void runAction("copy_artifact", "copied")}
          >
            <CopyIcon />Copy
          </button>
        )}
        <button
          type="button"
          disabled={busy !== null || isExiting}
          onClick={() => void runAction(chrome.hasPath ? "reveal_artifact" : "save_artifact", chrome.hasPath ? undefined : "saved")}
        >
          {chrome.feedback === "saved"
            ? <><CheckIcon />Saved</>
            : chrome.hasPath
              ? <><FolderIcon />Show in Folder</>
              : <><SaveIcon />Save file</>}
        </button>
      </div>
      <div className="thumbnail-bottom-bar">
        <div className="thumbnail-meta">
          <span>{artifact.width} × {artifact.height} · {formatFileSize(artifact.size_bytes)}</span>
          {!chrome.clipboardCurrent && !chrome.historySaved
            ? <span className="warning">Not in History</span>
            : !chrome.clipboardCurrent && chrome.copyFailed
              ? <span className="warning">Clipboard unavailable</span>
              : null}
        </div>
        <div className="thumbnail-status-chips">
          {chrome.clipboardCurrent && (
            <div className="clipboard-confirmation" role="status">
              <CheckIcon />
              <span>Copied to clipboard</span>
            </div>
          )}
        </div>
      </div>
      {error && <p className="thumbnail-message">{error}</p>}
    </article>
  );
}

function IconButton({
  children,
  className = "",
  label,
  disabled = false,
  onClick,
}: {
  children: React.ReactNode;
  className?: string;
  label: string;
  disabled?: boolean;
  onClick: () => void;
}) {
  const pressed = className.split(/\s+/).includes("active");
  return (
    <button
      type="button"
      className={`icon-button ${className}`.trim()}
      aria-label={label}
      aria-pressed={pressed || undefined}
      data-tooltip={label}
      disabled={disabled}
      onClick={disabled ? undefined : onClick}
    >
      {children}
    </button>
  );
}

function CopyIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="8" width="11" height="11" rx="2" /><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" /></svg>;
}

function FolderIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" /><circle cx="16.5" cy="13.5" r="2.5" /><path d="m18.3 15.3 2.2 2.2" /></svg>;
}

function EditIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m4 16-1 5 5-1L19 9l-4-4ZM13.5 6.5l4 4M4 16l4 4" /></svg>;
}

function TrashIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16M9 7V4h6v3m3 0-1 13H7L6 7m4 4v5m4-5v5" /></svg>;
}

function CloseIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18" /></svg>;
}

function SaveIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 4h12l2 2v14H5Z" /><path d="M8 4v6h8V4M8 20v-6h8v6" /></svg>;
}

function CheckIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m5 12 4 4L19 6" /></svg>;
}

function RecordingRecovery({
  drafts,
  onChanged,
}: {
  drafts: RecordingDraftManifest[];
  onChanged: () => Promise<void>;
}) {
  const [busyId, setBusyId] = useState<string | null>(null);
  const [confirmDiscardId, setConfirmDiscardId] = useState<string | null>(null);
  const [error, setError] = useState("");

  if (drafts.length === 0 && !error) return null;

  const run = async (command: "recover_recording_draft" | "discard_recording_draft", sessionId: string) => {
    if (busyId) return;
    setBusyId(sessionId);
    setError("");
    try {
      await invoke(command, { sessionId });
      setConfirmDiscardId(null);
      await onChanged();
    } catch (error) {
      setError(recordingErrorMessage(error));
      await onChanged().catch(() => undefined);
    } finally {
      setBusyId(null);
    }
  };

  return (
    <section className="recording-recovery-section">
      <h2>Interrupted recordings</h2>
      <p className="help-text">These recordings stopped before Captures could finish saving them. Recover one to add its playable segments to Capture History, or discard it.</p>
      {drafts.map((draft) => {
        const duration = draft.segments
          .filter((segment) => segment.complete)
          .reduce((total, segment) => total + segment.duration_ms, 0);
        const isBusy = busyId === draft.session_id;
        return (
          <div className="recording-recovery-row" key={draft.session_id}>
            <div>
              <strong>{draft.options.kind === "gif" ? "GIF" : "Video"} recording</strong>
              <small>{new Date(draft.created_at_ms).toLocaleString()} · {formatRecordingTime(duration)} recovered so far</small>
              {draft.last_error && (
                <small className="warning">{recordingErrorMessage(draft.last_error)}</small>
              )}
            </div>
            <div>
              <button type="button" disabled={Boolean(busyId)} onClick={() => void run("recover_recording_draft", draft.session_id)}>{isBusy ? "Recovering…" : "Recover"}</button>
              <button
                type="button"
                className={confirmDiscardId === draft.session_id ? "danger" : ""}
                disabled={Boolean(busyId)}
                onClick={() => {
                  if (confirmDiscardId === draft.session_id) {
                    void run("discard_recording_draft", draft.session_id);
                  } else {
                    setConfirmDiscardId(draft.session_id);
                  }
                }}
              >{confirmDiscardId === draft.session_id ? "Discard permanently?" : "Discard"}</button>
            </div>
          </div>
        );
      })}
      {error && <p className="settings-error" role="alert">{error}</p>}
    </section>
  );
}

type PreferencesSaveStatus = {
  kind: "idle" | "saving" | "saved" | "error";
  message: string;
};

const PREFERENCE_SECTIONS = [
  { id: "appearance", label: "Appearance" },
  { id: "capture", label: "Capture" },
  { id: "shortcuts", label: "Shortcuts" },
  { id: "recording", label: "Recording" },
  { id: "gif", label: "GIF export" },
  { id: "updates", label: "Updates" },
  { id: "about", label: "About" },
] as const;

type PreferenceSectionId = (typeof PREFERENCE_SECTIONS)[number]["id"];

/** Highlights the sidebar entry for whichever settings card is in view. */
function useVisibleSection(
  scrollerRef: RefObject<HTMLElement | null>,
): [PreferenceSectionId, (id: PreferenceSectionId) => void] {
  const [visible, setVisible] = useState<PreferenceSectionId>(PREFERENCE_SECTIONS[0].id);

  useEffect(() => {
    const root = scrollerRef.current;
    if (!root || typeof IntersectionObserver !== "function") return;
    const observer = new IntersectionObserver(
      (entries) => {
        const topMost = entries
          .filter((entry) => entry.isIntersecting)
          .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)[0];
        const id = topMost?.target.id as PreferenceSectionId | undefined;
        if (id) setVisible(id);
      },
      { root, rootMargin: "0px 0px -68% 0px", threshold: 0 },
    );
    for (const section of PREFERENCE_SECTIONS) {
      const element = root.querySelector(`#${section.id}`);
      if (element) observer.observe(element);
    }
    return () => observer.disconnect();
  }, [scrollerRef]);

  return [visible, setVisible];
}

const PREFERENCE_FIND_MATCH_CLASS = "preference-find-match";
const PREFERENCE_FIND_CURRENT_CLASS = "preference-find-current";

function usePreferencesFind(
  scrollerRef: RefObject<HTMLElement | null>,
  recordingShortcut: string | null,
  contentRevision: unknown,
) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [matchCount, setMatchCount] = useState(0);
  const [currentIndex, setCurrentIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const matchesRef = useRef<HTMLElement[]>([]);
  const scrolledKeyRef = useRef("");

  const clearHighlights = useCallback(() => {
    const root = scrollerRef.current;
    if (!root) return;
    for (const element of root.querySelectorAll(
      `.${PREFERENCE_FIND_MATCH_CLASS}, .${PREFERENCE_FIND_CURRENT_CLASS}`,
    )) {
      element.classList.remove(PREFERENCE_FIND_MATCH_CLASS, PREFERENCE_FIND_CURRENT_CLASS);
    }
  }, [scrollerRef]);

  const revealFind = useCallback(() => {
    setOpen(true);
    window.requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
  }, []);

  const closeFind = useCallback(() => {
    setOpen(false);
    clearHighlights();
    matchesRef.current = [];
    setMatchCount(0);
    scrolledKeyRef.current = "";
  }, [clearHighlights]);

  const goToMatch = useCallback((delta: 1 | -1) => {
    setCurrentIndex((current) => {
      const count = matchesRef.current.length;
      if (count === 0) return current;
      return wrapFindIndex(count, current, delta);
    });
  }, []);

  const updateQuery = useCallback((value: string) => {
    setQuery(value);
    setCurrentIndex(0);
    scrolledKeyRef.current = "";
    const root = scrollerRef.current;
    const matches = root
      ? matchPreferenceFindTargets(collectPreferenceFindTargets(root), value)
      : [];
    matchesRef.current = matches;
    setMatchCount(matches.length);
  }, [scrollerRef]);

  useLayoutEffect(() => {
    if (!open) {
      clearHighlights();
      matchesRef.current = [];
      scrolledKeyRef.current = "";
      return;
    }
    const root = scrollerRef.current;
    if (!root) return;
    clearHighlights();
    const matches = matchPreferenceFindTargets(collectPreferenceFindTargets(root), query);
    matchesRef.current = matches;
    if (matches.length === 0) return;
    const index = Math.min(currentIndex, matches.length - 1);
    for (const [matchIndex, match] of matches.entries()) {
      match.classList.add(PREFERENCE_FIND_MATCH_CLASS);
      if (matchIndex === index) match.classList.add(PREFERENCE_FIND_CURRENT_CLASS);
    }
    const current = matches[index];
    const key = `${query}\0${index}\0${current.textContent ?? ""}`;
    if (scrolledKeyRef.current === key) return;
    scrolledKeyRef.current = key;
    if (typeof current.scrollIntoView === "function") {
      current.scrollIntoView({ behavior: "smooth", block: "center" });
    }
  }, [clearHighlights, contentRevision, currentIndex, open, query, scrollerRef]);

  useEffect(() => {
    if (!open) return;
    const frame = window.requestAnimationFrame(() => {
      const root = scrollerRef.current;
      const matches = root
        ? matchPreferenceFindTargets(collectPreferenceFindTargets(root), query)
        : [];
      matchesRef.current = matches;
      setMatchCount(matches.length);
      setCurrentIndex((current) => (
        matches.length === 0 ? 0 : Math.min(current, matches.length - 1)
      ));
    });
    return () => window.cancelAnimationFrame(frame);
  }, [contentRevision, open, query, scrollerRef]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (recordingShortcut) return;
      const command = preferencesFindCommand(
        event,
        detectShortcutPlatform(),
        open,
      );
      if (!command) return;
      event.preventDefault();
      if (command === "open") {
        revealFind();
        return;
      }
      if (command === "close") {
        closeFind();
        return;
      }
      goToMatch(command === "next" ? 1 : -1);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [closeFind, goToMatch, open, recordingShortcut, revealFind]);

  return {
    open,
    query,
    matchCount,
    currentIndex,
    countLabel: preferenceFindCountLabel(query, matchCount, currentIndex),
    inputRef,
    updateQuery,
    closeFind,
    goToMatch,
  };
}

function PreferencesFindBar({
  query,
  countLabel,
  matchCount,
  inputRef,
  onQueryChange,
  onClose,
  onNext,
  onPrevious,
}: {
  query: string;
  countLabel: string;
  matchCount: number;
  inputRef: RefObject<HTMLInputElement | null>;
  onQueryChange: (value: string) => void;
  onClose: () => void;
  onNext: () => void;
  onPrevious: () => void;
}) {
  return (
    <div className="preferences-find" role="search">
      <input
        ref={inputRef}
        type="search"
        className="preferences-find-input"
        value={query}
        placeholder="Find settings"
        aria-label="Find settings"
        autoComplete="off"
        autoCorrect="off"
        spellCheck={false}
        onChange={(event) => onQueryChange(event.target.value)}
      />
      <span className="preferences-find-count" aria-live="polite">{countLabel}</span>
      <button
        type="button"
        className="preferences-find-step"
        aria-label="Previous match"
        disabled={matchCount === 0}
        onClick={onPrevious}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 10 4-4 4 4" /></svg>
      </button>
      <button
        type="button"
        className="preferences-find-step"
        aria-label="Next match"
        disabled={matchCount === 0}
        onClick={onNext}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 6 4 4 4-4" /></svg>
      </button>
      <button
        type="button"
        className="preferences-find-close"
        aria-label="Close find"
        onClick={onClose}
      >
        <CloseIcon />
      </button>
    </div>
  );
}

function MiniPreviewPlacementPicker({
  value,
  disabled,
  onChange,
}: {
  value: MiniPreviewPlacement;
  disabled?: boolean;
  onChange: (placement: MiniPreviewPlacement) => void;
}) {
  const selected = MINI_PREVIEW_PLACEMENTS.find((placement) => placement.id === value)
    ?? MINI_PREVIEW_PLACEMENTS[2];
  return (
    <div className={`mini-preview-placement${disabled ? " is-disabled" : ""}`}>
      <div
        className="mini-preview-placement-screen"
        role="radiogroup"
        aria-label="Mini preview position"
        aria-disabled={disabled || undefined}
      >
        {MINI_PREVIEW_PLACEMENTS.map((placement) => (
          <button
            key={placement.id}
            type="button"
            className={`mini-preview-placement-corner mini-preview-placement-${placement.id}${
              value === placement.id ? " active" : ""
            }`}
            role="radio"
            aria-checked={value === placement.id}
            aria-label={placement.name}
            disabled={disabled}
            onClick={() => onChange(placement.id)}
          />
        ))}
      </div>
      <small>{selected.name}</small>
    </div>
  );
}

function SettingRow({
  title,
  description,
  control,
  layout = "inline",
}: {
  title: React.ReactNode;
  description?: React.ReactNode;
  control: React.ReactNode;
  /** `stack` puts the control on its own line for wide inputs. */
  layout?: "inline" | "stack";
}) {
  return (
    <div className={`setting-row setting-row-${layout}`}>
      <div className="setting-copy">
        <span>{title}</span>
        {description && <small>{description}</small>}
      </div>
      <div className="setting-control">{control}</div>
    </div>
  );
}

function ThemeColorField({
  label,
  description,
  value,
  onChange,
}: {
  label: string;
  description: string;
  value: string;
  onChange: (value: string) => void;
}) {
  const pickerId = useId();
  const commitHexValue = (input: HTMLInputElement) => {
    const normalized = normalizeHexColor(input.value);
    if (normalized) onChange(normalized);
    else input.value = value.toUpperCase();
  };

  return (
    <div className="custom-theme-field">
      <label htmlFor={pickerId}>{label}</label>
      <div className="custom-theme-color-control">
        <input
          id={pickerId}
          type="color"
          value={value}
          aria-label={`${label} color picker`}
          onChange={(event) => onChange(event.target.value)}
        />
        <input
          key={value}
          type="text"
          defaultValue={value.toUpperCase()}
          aria-label={`${label} hex value`}
          maxLength={7}
          spellCheck={false}
          onBlur={(event) => commitHexValue(event.currentTarget)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              event.currentTarget.blur();
            } else if (event.key === "Escape") {
              event.preventDefault();
              event.currentTarget.value = value.toUpperCase();
              event.currentTarget.blur();
            }
          }}
        />
      </div>
      <small>{description}</small>
    </div>
  );
}

export function Preferences() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [canExcludeRecordingControls, setCanExcludeRecordingControls] = useState(true);
  const [recordingDevices, setRecordingDevices] = useState<AudioDevice[]>([]);
  const [saveStatus, setSaveStatus] = useState<PreferencesSaveStatus>({ kind: "idle", message: "" });
  const [recordingShortcut, setRecordingShortcut] = useState<string | null>(null);
  const [requestedPreferenceTarget, setRequestedPreferenceTarget] = useState<string | null>(
    () => query("target"),
  );
  const [highlightedPreference, setHighlightedPreference] = useState<string | null>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const [visibleSection, setVisibleSection] = useVisibleSection(scrollerRef);
  const find = usePreferencesFind(scrollerRef, recordingShortcut, settings);
  const settingsRef = useRef<AppSettings | null>(null);
  const pendingSettingsRef = useRef<AppSettings | null>(null);
  const saveDelayTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savedStatusTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveInFlightRef = useRef<Promise<void> | null>(null);
  const activeRef = useRef(true);
  const preferenceHighlightTimerRef = useRef<number | null>(null);

  useEffect(() => {
    let active = true;
    let dispose: (() => void) | undefined;
    void listen<string>(PREFERENCES_TARGET_EVENT, ({ payload }) => {
      if (active) setRequestedPreferenceTarget(payload);
    }).then((unlisten) => {
      if (active) dispose = unlisten;
      else unlisten();
    }).catch(() => undefined);
    return () => {
      active = false;
      dispose?.();
    };
  }, []);

  const setShortcutRecording = useCallback((id: string, recording: boolean) => {
    setRecordingShortcut(recording ? id : null);
    void invoke("set_shortcut_capture_suppressed", {
      suppressed: recording,
    }).catch(() => undefined);
  }, []);

  useEffect(() => () => {
    void invoke("set_shortcut_capture_suppressed", {
      suppressed: false,
    }).catch(() => undefined);
  }, []);

  const clearSavedStatusTimer = useCallback(() => {
    if (!savedStatusTimerRef.current) return;
    clearTimeout(savedStatusTimerRef.current);
    savedStatusTimerRef.current = null;
  }, []);

  const updateSaveStatus = useCallback((status: PreferencesSaveStatus) => {
    if (activeRef.current) setSaveStatus(status);
  }, []);

  const flushPendingSettings = useCallback(async (): Promise<void> => {
    if (saveDelayTimerRef.current) {
      clearTimeout(saveDelayTimerRef.current);
      saveDelayTimerRef.current = null;
    }
    while (true) {
      if (saveInFlightRef.current) await saveInFlightRef.current;
      if (saveDelayTimerRef.current) return;
      const pendingSettings = pendingSettingsRef.current;
      if (!pendingSettings) return;
      pendingSettingsRef.current = null;
      updateSaveStatus({ kind: "saving", message: "Saving changes…" });

      const request = (async () => {
        try {
          const saved = await invoke<AppSettings>("update_settings", { settings: pendingSettings });
          if (!pendingSettingsRef.current) {
            settingsRef.current = saved;
            if (activeRef.current) {
              setSettings(saved);
              updateSaveStatus({ kind: "saved", message: "Changes saved" });
              clearSavedStatusTimer();
              savedStatusTimerRef.current = setTimeout(() => {
                updateSaveStatus({ kind: "idle", message: "" });
                savedStatusTimerRef.current = null;
              }, 2_000);
            }
          }
        } catch (error) {
          if (!pendingSettingsRef.current) {
            updateSaveStatus({ kind: "error", message: `Couldn’t save changes: ${String(error)}` });
          }
        }
      })();
      saveInFlightRef.current = request;
      await request;
      if (saveInFlightRef.current === request) saveInFlightRef.current = null;
    }
  }, [clearSavedStatusTimer, updateSaveStatus]);

  const scheduleSettingsSave = (nextSettings: AppSettings) => {
    pendingSettingsRef.current = nextSettings;
    clearSavedStatusTimer();
    updateSaveStatus({ kind: "saving", message: "Saving changes…" });
    if (saveDelayTimerRef.current) clearTimeout(saveDelayTimerRef.current);
    saveDelayTimerRef.current = setTimeout(() => {
      saveDelayTimerRef.current = null;
      void flushPendingSettings();
    }, 250);
  };

  useEffect(() => {
    let active = true;
    activeRef.current = true;
    void Promise.all([
      invoke<AppSettings>("get_settings"),
      invoke<boolean>("platform_can_exclude_recording_controls").catch(() => true),
    ]).then(([loadedSettings, canExclude]) => {
      if (!active) return;
      settingsRef.current = loadedSettings;
      setSettings(loadedSettings);
      setCanExcludeRecordingControls(canExclude);
    });
    return () => {
      active = false;
      activeRef.current = false;
      clearSavedStatusTimer();
      if (saveDelayTimerRef.current) {
        clearTimeout(saveDelayTimerRef.current);
        saveDelayTimerRef.current = null;
        void flushPendingSettings();
      }
    };
  }, [clearSavedStatusTimer, flushPendingSettings]);

  useEffect(() => {
    let active = true;
    void invoke<AudioDevice[]>("list_recording_audio_devices")
      .then((devices) => {
        if (active) setRecordingDevices(devices);
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (!settings || !requestedPreferenceTarget) return;
    const elementId = PREFERENCE_TARGET_IDS[requestedPreferenceTarget];
    if (!elementId) return;
    const targetName = requestedPreferenceTarget;
    const frame = window.requestAnimationFrame(() => {
      setVisibleSection("capture");
      const target = scrollerRef.current?.querySelector<HTMLElement>(`#${elementId}`);
      if (!target) return;
      if (typeof target.scrollIntoView === "function") {
        target.scrollIntoView({ behavior: "smooth", block: "center" });
      }
      target.querySelector<HTMLInputElement>("input")?.focus({ preventScroll: true });
      setHighlightedPreference(targetName);
      if (preferenceHighlightTimerRef.current) {
        window.clearTimeout(preferenceHighlightTimerRef.current);
      }
      preferenceHighlightTimerRef.current = window.setTimeout(() => {
        setHighlightedPreference(null);
        setRequestedPreferenceTarget(null);
        preferenceHighlightTimerRef.current = null;
      }, PREFERENCE_HIGHLIGHT_MS);
    });
    return () => window.cancelAnimationFrame(frame);
  }, [requestedPreferenceTarget, setVisibleSection, settings]);

  useEffect(() => () => {
    if (preferenceHighlightTimerRef.current) {
      window.clearTimeout(preferenceHighlightTimerRef.current);
    }
  }, []);

  if (!settings) return <main className="preferences loading">Loading preferences…</main>;

  const update = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => {
    const current = settingsRef.current;
    if (!current || Object.is(current[key], value)) return;
    const next = { ...current, [key]: value };
    settingsRef.current = next;
    setSettings(next);
    scheduleSettingsSave(next);
  };

  const updateRecording = <K extends keyof AppSettings["recording"]>(
    key: K,
    value: AppSettings["recording"][K],
  ) => {
    const current = settingsRef.current;
    if (!current || Object.is(current.recording[key], value)) return;
    const next = { ...current, recording: { ...current.recording, [key]: value } };
    settingsRef.current = next;
    setSettings(next);
    scheduleSettingsSave(next);
  };

  const setCustomTheme = (colors: CustomThemeColors) => {
    const current = settingsRef.current;
    if (!current) return;
    const customTheme = normalizeCustomThemeColors(colors);
    const next: AppSettings = {
      ...current,
      theme: "custom",
      custom_theme: customTheme,
    };
    applyColorTheme("custom", customTheme);
    settingsRef.current = next;
    setSettings(next);
    scheduleSettingsSave(next);
  };

  const chooseDirectory = async () => {
    const selected = await open({ directory: true, multiple: false, title: "Choose capture folder" });
    if (typeof selected === "string") update("output_directory", selected);
  };

  const setAppearance = (mode: AppearanceMode) => {
    applyAppearance(mode);
    update("appearance", mode);
  };

  const goToSection = (id: PreferenceSectionId) => {
    setVisibleSection(id);
    const target = scrollerRef.current?.querySelector(`#${id}`);
    if (target && typeof target.scrollIntoView === "function") {
      target.scrollIntoView({ behavior: "smooth", block: "start" });
    }
  };

  return (
    <main className="preferences">
      <aside className="preferences-nav">
        <div className="preferences-nav-brand">
          <span aria-hidden="true"><CaptureIcon /></span>
          <strong>Captures</strong>
        </div>
        <nav aria-label="Preferences sections">
          {PREFERENCE_SECTIONS.map((section) => (
            <button
              key={section.id}
              type="button"
              className={visibleSection === section.id ? "active" : ""}
              aria-current={visibleSection === section.id ? "true" : undefined}
              onClick={() => goToSection(section.id)}
            >
              {section.label}
            </button>
          ))}
        </nav>
      </aside>

      <div className="preferences-body">
        <header className="preferences-header">
          <div>
            <h1>Preferences</h1>
            <p>Changes save automatically.</p>
          </div>
          <div className="preferences-header-actions">
            <button
              type="button"
              className="preferences-history-button"
              onClick={() => void invoke("open_capture_history")}
            >
              Capture History…
            </button>
            {saveStatus.kind !== "idle" && (
              <div className={`preferences-save-status preferences-save-${saveStatus.kind}`} role="status">
                <span aria-hidden="true">{saveStatus.kind === "saved" ? "✓" : saveStatus.kind === "error" ? "!" : ""}</span>
                {saveStatus.message}
              </div>
            )}
          </div>
          {find.open && (
            <PreferencesFindBar
              query={find.query}
              countLabel={find.countLabel}
              matchCount={find.matchCount}
              inputRef={find.inputRef}
              onQueryChange={find.updateQuery}
              onClose={find.closeFind}
              onNext={() => find.goToMatch(1)}
              onPrevious={() => find.goToMatch(-1)}
            />
          )}
        </header>

        <div className="preferences-scroller" ref={scrollerRef}>
          <div className="preferences-sections">
            <PreferencesSections
              settings={settings}
              canExcludeRecordingControls={canExcludeRecordingControls}
              highlightedPreference={highlightedPreference}
              recordingDevices={recordingDevices}
              recordingShortcut={recordingShortcut}
              setRecordingShortcut={setRecordingShortcut}
              setShortcutRecording={setShortcutRecording}
              update={update}
              updateRecording={updateRecording}
              setAppearance={setAppearance}
              setCustomTheme={setCustomTheme}
              chooseDirectory={chooseDirectory}
            />
          </div>
        </div>
      </div>
    </main>
  );
}

function PreferencesSections({
  settings,
  canExcludeRecordingControls,
  highlightedPreference,
  recordingDevices,
  recordingShortcut,
  setRecordingShortcut,
  setShortcutRecording,
  update,
  updateRecording,
  setAppearance,
  setCustomTheme,
  chooseDirectory,
}: {
  settings: AppSettings;
  canExcludeRecordingControls: boolean;
  highlightedPreference: string | null;
  recordingDevices: AudioDevice[];
  recordingShortcut: string | null;
  setRecordingShortcut: (id: string | null) => void;
  setShortcutRecording: (id: string, recording: boolean) => void;
  update: <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => void;
  updateRecording: <K extends keyof AppSettings["recording"]>(
    key: K,
    value: AppSettings["recording"][K],
  ) => void;
  setAppearance: (mode: AppearanceMode) => void;
  setCustomTheme: (colors: CustomThemeColors) => void;
  chooseDirectory: () => Promise<void>;
}) {
  const appearance = settings.appearance ?? DEFAULT_APPEARANCE;
  const shortcutHelp = platformShortcutHelp(detectShortcutPlatform());

  return (
    <>
      <section className="settings-card" id="appearance" aria-labelledby="appearance-heading">
        <header className="settings-card-header">
          <h2 id="appearance-heading">Appearance</h2>
          <p>One look across every Captures window. Capture overlays stay dark so they read on any desktop.</p>
        </header>

        <SettingRow
          title="Interface theme"
          description="Follow the system setting, or lock Captures to light or dark."
          control={(
            <div className="ui-segmented appearance-switch" role="group" aria-label="Interface theme" data-active={appearance}>
              <SegmentedControlIndicator value={appearance} />
              {APPEARANCE_MODES.map((mode) => (
                <button
                  key={mode.id}
                  type="button"
                  className={appearance === mode.id ? "active" : ""}
                  aria-pressed={appearance === mode.id}
                  onClick={() => setAppearance(mode.id)}
                >
                  {mode.name}
                </button>
              ))}
            </div>
          )}
        />

        <SettingRow
          layout="stack"
          title="Accent color"
          description="Used for the capture action, selection, and focus. Status colors keep their meaning."
          control={(
            <div className="theme-options" role="radiogroup" aria-label="Color theme">
              {COLOR_THEMES.map((theme) => {
                const previewStyle = theme.id === "custom"
                  ? buildCustomThemeVariables(settings.custom_theme) as CSSProperties
                  : undefined;
                return (
                  <button
                    key={theme.id}
                    type="button"
                    className={`theme-option theme-option-${theme.id}${settings.theme === theme.id ? " active" : ""}`}
                    role="radio"
                    aria-checked={settings.theme === theme.id}
                    aria-label={`${theme.name}: ${theme.description}`}
                    data-capture-theme={theme.id}
                    style={previewStyle}
                    title={theme.description}
                    onClick={() => {
                      applyColorTheme(theme.id, settings.custom_theme);
                      update("theme", theme.id);
                    }}
                  >
                    <span className="theme-option-preview" aria-hidden="true">
                      <span />
                      <span />
                    </span>
                    <span className="theme-option-copy">
                      <strong>{theme.name}</strong>
                      <small>{theme.description}</small>
                    </span>
                    <span className="theme-option-check" aria-hidden="true">✓</span>
                  </button>
                );
              })}
            </div>
          )}
        />

        {settings.theme === "custom" && (
          <div className="custom-theme-editor" role="group" aria-label="Custom theme colors">
            <div className="custom-theme-editor-heading">
              <div>
                <strong>Custom colors</strong>
                <small>Open either RGB picker or enter a hex value. Supporting shades stay readable.</small>
              </div>
              <button
                type="button"
                onClick={() => setCustomTheme({ ...DEFAULT_CUSTOM_THEME })}
              >
                Reset colors
              </button>
            </div>
            <div className="custom-theme-fields">
              <ThemeColorField
                label="Accent"
                description="Capture actions, selections, focus, and editing."
                value={settings.custom_theme.accent}
                onChange={(accent) => setCustomTheme({ ...settings.custom_theme, accent })}
              />
              <ThemeColorField
                label="Recording signal"
                description="Recording indicators, errors, and destructive actions."
                value={settings.custom_theme.signal}
                onChange={(signal) => setCustomTheme({ ...settings.custom_theme, signal })}
              />
            </div>
          </div>
        )}
      </section>

      <section className="settings-card" id="capture" aria-labelledby="capture-heading">
        <header className="settings-card-header">
          <h2 id="capture-heading">Capture</h2>
          <p>Where captures go and what happens right after you take one.</p>
        </header>

        <SettingRow
          layout="stack"
          title="Save captures to"
          control={(
            <div className="directory-input">
              <input
                id="output-directory"
                aria-label="Save captures to"
                value={settings.output_directory}
                onChange={(event) => update("output_directory", event.target.value)}
              />
              <button type="button" onClick={() => void chooseDirectory()}>Choose…</button>
            </div>
          )}
        />

        <label className="check-row switch-row">
          <input
            type="checkbox"
            checked={settings.auto_copy_to_clipboard}
            onChange={(event) => update("auto_copy_to_clipboard", event.target.checked)}
          />
          <span>
            Automatically copy captures to the clipboard
            <small>Turn this off to preserve existing text or other clipboard contents.</small>
          </span>
        </label>

        <label
          id={AUTO_START_PREFERENCE_ID}
          className={`check-row switch-row${highlightedPreference === AUTO_START_PREFERENCE_TARGET
            ? " preference-target-highlight"
            : ""}`}
        >
          <input
            type="checkbox"
            checked={settings.auto_start_on_selection}
            onChange={(event) => update("auto_start_on_selection", event.target.checked)}
          />
          <span>
            Start capture as soon as a target is selected
            <small>
              Drawing a region, choosing a window, or clicking Full screen
              immediately starts the capture. When this is off, press Enter
              in the capture menu to confirm.
            </small>
          </span>
        </label>

        <label className="check-row switch-row">
          <input
            type="checkbox"
            checked={settings.show_mini_previews}
            onChange={(event) => update("show_mini_previews", event.target.checked)}
          />
          <span>
            Show mini previews after screenshots
            <small>Turn this off to keep the quick-access preview stack hidden.</small>
          </span>
        </label>

        <SettingRow
          title="Mini preview position"
          description="Choose a screen corner. Show less stays on that edge, and the stack opens away from it. You can still drag the collapsed pile during a session."
          control={(
            <MiniPreviewPlacementPicker
              value={settings.mini_preview_placement ?? DEFAULT_MINI_PREVIEW_PLACEMENT}
              disabled={!settings.show_mini_previews}
              onChange={(placement) => update("mini_preview_placement", placement)}
            />
          )}
        />

        <label className="check-row switch-row">
          <input
            type="checkbox"
            checked={settings.include_mini_previews_in_captures}
            onChange={(event) => update("include_mini_previews_in_captures", event.target.checked)}
            disabled={!settings.show_mini_previews}
          />
          <span>
            Show mini previews in screenshots and recordings
            <small>
              {!settings.show_mini_previews
                ? "Mini previews are off, so they won’t show in screenshots or recordings."
                : settings.include_mini_previews_in_captures
                  ? "Mini previews will show in screenshots and recordings. Turn this off to keep them out."
                  : "Mini previews won’t show in screenshots or recordings."}
            </small>
          </span>
        </label>

        <label
          id={RECORDING_CONTROLS_PREFERENCE_ID}
          className={`check-row switch-row${highlightedPreference === RECORDING_CONTROLS_PREFERENCE_TARGET
            ? " preference-target-highlight"
            : ""}`}
        >
          <input
            type="checkbox"
            checked={settings.include_recording_controls_in_captures}
            onChange={(event) => update("include_recording_controls_in_captures", event.target.checked)}
            disabled={!canExcludeRecordingControls}
          />
          <span>
            Show recording controls in screenshots and recordings
            <small>
              {!canExcludeRecordingControls
                ? "This desktop session cannot keep recording controls out of screenshots and recordings. Use Hide controls on the recording bar to keep them off-screen."
                : settings.include_recording_controls_in_captures
                  ? <>Recording controls <strong>will</strong> show in screenshots and recordings. Turn this off to keep them out.</>
                  : <>Recording controls <strong>won’t</strong> show in screenshots or recordings.</>}
            </small>
          </span>
        </label>

        <label className="check-row switch-row">
          <input
            type="checkbox"
            checked={settings.freeze_screen}
            onChange={(event) => update("freeze_screen", event.target.checked)}
          />
          <span>
            Freeze screen when capturing
            <small>
              Holds hover states, tooltips, menus, and motion still while you choose a region or window.
              Turn this off to select from the live desktop.
            </small>
          </span>
        </label>

        <label className="check-row switch-row">
          <input
            type="checkbox"
            checked={settings.show_cursor_in_screenshots}
            onChange={(event) => update("show_cursor_in_screenshots", event.target.checked)}
          />
          <span>
            Show cursor in screenshots
            <small>
              Includes the pointer in still captures. Freeze screen only holds the desktop still; it
              does not add the cursor by itself.
            </small>
          </span>
        </label>

        <SettingRow
          title="Screenshot format"
          description="Used when you save or export. Capture History keeps a lossless PNG until then."
          control={(
            <CustomSelect
              value={settings.screenshot_format}
              ariaLabel="Screenshot format"
              options={[
                { value: "png", label: "PNG" },
                { value: "jpeg", label: "JPEG" },
                { value: "webp", label: "WebP" },
              ]}
              onChange={(value) => update("screenshot_format", value as ScreenshotFormat)}
            />
          )}
        />

        <SettingRow
          title="Screenshot countdown"
          description="Wait before capturing so you can open menus or hover states. Press Esc to cancel."
          control={(
            <CustomSelect
              value={String(settings.screenshot_countdown_seconds)}
              ariaLabel="Screenshot countdown"
              options={COUNTDOWN_SECONDS.map((value) => ({
                value: String(value),
                label: value === 0 ? "Off" : value === 1 ? "1 second" : `${value} seconds`,
              }))}
              onChange={(value) => update("screenshot_countdown_seconds", Number(value))}
            />
          )}
        />
      </section>

      <section className="settings-card" id="shortcuts" aria-labelledby="shortcuts-heading">
        <header className="settings-card-header">
          <h2 id="shortcuts-heading">Shortcuts</h2>
          <p>
            Select a shortcut, then press the key combination you want. Press Esc to cancel recording.
            {` ${shortcutHelp.intro}`}
          </p>
        </header>
        <div className="settings-utility-row">
          <div className="settings-utility-copy">
            <strong>{shortcutHelp.takeoverTitle}</strong>
            <small>{shortcutHelp.takeoverBody}</small>
          </div>
          <button
            className="settings-utility-action"
            type="button"
            onClick={() => void invoke("open_system_screenshot_shortcut_settings")}
          >
            Open
          </button>
        </div>
        <div className="shortcut-list">
          <ShortcutInput
            id="new-capture-shortcut"
            label="New Capture"
            value={settings.new_capture_shortcut}
            recording={recordingShortcut === "new-capture-shortcut"}
            onRecordingChange={(recording) => setRecordingShortcut(recording ? "new-capture-shortcut" : null)}
            onChange={(value) => update("new_capture_shortcut", value)}
          />
          <ShortcutInput
            id="region-shortcut"
            label="Region"
            value={settings.region_shortcut}
            recording={recordingShortcut === "region-shortcut"}
            onRecordingChange={(recording) => setShortcutRecording("region-shortcut", recording)}
            onChange={(value) => update("region_shortcut", value)}
          />
          <ShortcutInput
            id="window-shortcut"
            label="Window"
            value={settings.window_shortcut}
            recording={recordingShortcut === "window-shortcut"}
            onRecordingChange={(recording) => setShortcutRecording("window-shortcut", recording)}
            onChange={(value) => update("window_shortcut", value)}
          />
          <ShortcutInput
            id="display-shortcut"
            label="Full Screen"
            value={settings.display_shortcut}
            recording={recordingShortcut === "display-shortcut"}
            onRecordingChange={(recording) => setShortcutRecording("display-shortcut", recording)}
            onChange={(value) => update("display_shortcut", value)}
          />
          <ShortcutInput
            id="record-region-shortcut"
            label="Record Region"
            value={settings.recording.video_shortcut}
            recording={recordingShortcut === "record-region-shortcut"}
            onRecordingChange={(recording) => setShortcutRecording("record-region-shortcut", recording)}
            onChange={(value) => updateRecording("video_shortcut", value)}
          />
          <ShortcutInput
            id="record-window-shortcut"
            label="Record Window"
            value={settings.recording.window_shortcut}
            recording={recordingShortcut === "record-window-shortcut"}
            onRecordingChange={(recording) => setShortcutRecording("record-window-shortcut", recording)}
            onChange={(value) => updateRecording("window_shortcut", value)}
          />
          <ShortcutInput
            id="record-display-shortcut"
            label="Record Full Screen"
            value={settings.recording.display_shortcut}
            recording={recordingShortcut === "record-display-shortcut"}
            onRecordingChange={(recording) => setShortcutRecording("record-display-shortcut", recording)}
            onChange={(value) => updateRecording("display_shortcut", value)}
          />
        </div>
      </section>

      <section className="settings-card" id="recording" aria-labelledby="recording-heading">
        <header className="settings-card-header">
          <h2 id="recording-heading">Recording</h2>
          <p>Defaults for new screen recordings. You can still change them in the capture menu.</p>
        </header>

        <SettingRow
          title="Recording format"
          description="Recordings are captured as H.264 MP4. GIF and WebM are converted when you save or export."
          control={(
            <CustomSelect
              value={settings.recording.video_format}
              ariaLabel="Recording format"
              options={[
                { value: "mp4", label: "MP4" },
                { value: "gif", label: "GIF" },
                { value: "webm", label: "WebM" },
              ]}
              onChange={(value) => updateRecording("video_format", value as VideoFormat)}
            />
          )}
        />

        <div className="setting-grid">
          <SettingRow
            layout="stack"
            title="Frames per second"
            control={(
              <CustomSelect
                value={String(settings.recording.video_fps)}
                ariaLabel="Recording frames per second"
                options={[60, 30, 15].map((value) => ({ value: String(value), label: `${value} FPS` }))}
                onChange={(value) => updateRecording("video_fps", Number(value))}
              />
            )}
          />
          <SettingRow
            layout="stack"
            title="Maximum resolution"
            control={(
              <CustomSelect
                value={settings.recording.video_max_resolution}
                ariaLabel="Recording maximum resolution"
                options={[
                  { value: "original", label: "Original" },
                  { value: "p1080", label: "1080p" },
                  { value: "p720", label: "720p" },
                ]}
                onChange={(value) => updateRecording("video_max_resolution", value as MaxResolution)}
              />
            )}
          />
        </div>

        <SettingRow
          title="Countdown"
          description="Delay before a recording starts."
          control={(
            <CustomSelect
              value={String(settings.recording.countdown_seconds)}
              ariaLabel="Recording countdown"
              options={COUNTDOWN_SECONDS.map((value) => ({
                value: String(value),
                label: value === 0 ? "Off" : value === 1 ? "1 second" : `${value} seconds`,
              }))}
              onChange={(value) => updateRecording("countdown_seconds", Number(value))}
            />
          )}
        />

        <SettingRow
          title="Default microphone"
          description="Used when a recording starts with microphone audio."
          control={(
            <CustomSelect
              value={settings.recording.microphone_device_id ?? "off"}
              ariaLabel="Default microphone"
              options={[
                { value: "off", label: "Off" },
                ...recordingDevices.map((device) => ({ value: device.id, label: device.name })),
              ]}
              onChange={(value) => updateRecording("microphone_device_id", value === "off" ? null : value)}
            />
          )}
        />

        <label className="check-row switch-row">
          <input
            type="checkbox"
            checked={settings.recording.capture_system_audio}
            onChange={(event) => updateRecording("capture_system_audio", event.target.checked)}
          />
          <span>
            Record desktop audio
            <small>Records sound playing through the system output.</small>
          </span>
        </label>
        <label className="check-row switch-row">
          <input
            type="checkbox"
            checked={settings.recording.mono_audio}
            onChange={(event) => updateRecording("mono_audio", event.target.checked)}
          />
          <span>Export recording audio in mono</span>
        </label>
        <label className="check-row switch-row">
          <input
            type="checkbox"
            checked={settings.recording.show_cursor}
            onChange={(event) => updateRecording("show_cursor", event.target.checked)}
          />
          <span>Show cursor in recordings</span>
        </label>
        <label className="check-row switch-row">
          <input
            type="checkbox"
            checked={settings.recording.highlight_clicks}
            onChange={(event) => updateRecording("highlight_clicks", event.target.checked)}
          />
          <span>Show clicks in recordings</span>
        </label>
        <label className="check-row switch-row">
          <input
            type="checkbox"
            checked={settings.recording.open_editor_after_recording}
            onChange={(event) => updateRecording("open_editor_after_recording", event.target.checked)}
          />
          <span>
            Open the editor after recording
            <small>The recording is kept in Capture History for 30 days, so closing the editor never loses it.</small>
          </span>
        </label>
      </section>

      <section className="settings-card" id="gif" aria-labelledby="gif-heading">
        <header className="settings-card-header">
          <h2 id="gif-heading">GIF export</h2>
          <p>Starting point when a recording is exported as an animated GIF.</p>
        </header>
        <div className="setting-grid setting-grid-three">
          <SettingRow
            layout="stack"
            title="Frames per second"
            control={(
              <CustomSelect
                value={String(settings.recording.gif_fps)}
                ariaLabel="GIF frames per second"
                options={[8, 10, 12, 15, 20, 24, 30].map((value) => ({ value: String(value), label: `${value} FPS` }))}
                onChange={(value) => updateRecording("gif_fps", Number(value))}
              />
            )}
          />
          <SettingRow
            layout="stack"
            title="Maximum width"
            control={(
              <CustomSelect
                value={String(settings.recording.gif_max_width)}
                ariaLabel="GIF maximum width"
                options={[320, 480, 640, 800, 1200].map((value) => ({ value: String(value), label: `${value} px` }))}
                onChange={(value) => updateRecording("gif_max_width", Number(value))}
              />
            )}
          />
          <SettingRow
            layout="stack"
            title="Palette colors"
            control={(
              <CustomSelect
                value={String(settings.recording.gif_max_colors)}
                ariaLabel="GIF palette colors"
                options={[64, 96, 128, 256].map((value) => ({ value: String(value), label: String(value) }))}
                onChange={(value) => updateRecording("gif_max_colors", Number(value))}
              />
            )}
          />
        </div>
      </section>

      <UpdatePreferences
        showChangelog={settings.show_update_changelog !== false}
        updateShowChangelog={(value) => update("show_update_changelog", value)}
      />

      <section className="settings-card" id="about" aria-labelledby="about-heading">
        <header className="settings-card-header">
          <h2 id="about-heading">About</h2>
          <p>Captures is in active development. Telling us what breaks is the fastest way to fix it.</p>
        </header>
        <div className="settings-utility-row">
          <div className="settings-utility-copy">
            <strong>Send feedback</strong>
            <small>Report a bug or share an idea.</small>
          </div>
          <button className="settings-utility-action" type="button" onClick={() => void invoke("open_feedback")}>
            Open
          </button>
        </div>
        <label className="check-row switch-row">
          <input
            type="checkbox"
            checked={settings.launch_at_login}
            onChange={(event) => update("launch_at_login", event.target.checked)}
          />
          <span>Launch Captures when I sign in</span>
        </label>
      </section>
    </>
  );
}

export function ShortcutInput({
  id,
  label,
  value,
  recording,
  onRecordingChange,
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  recording: boolean;
  onRecordingChange: (recording: boolean) => void;
  onChange: (value: string) => void;
}) {
  const recorderRef = useRef<HTMLButtonElement>(null);
  const [previewKeys, setPreviewKeys] = useState<string[]>([]);
  const [error, setError] = useState("");
  const keys = recording ? previewKeys : shortcutDisplayTokens(value);

  useEffect(() => {
    // WKWebView follows Safari's macOS behavior and does not reliably focus a
    // button when it is clicked. The recorder only receives keyboard events
    // while focused, so acquire focus explicitly when recording begins.
    if (recording) recorderRef.current?.focus();
  }, [recording]);

  const stopRecording = () => {
    setPreviewKeys([]);
    setError("");
    onRecordingChange(false);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!recording) return;
    event.preventDefault();
    event.stopPropagation();
    const result = recordShortcut(event);
    if (result.kind === "cancel") {
      stopRecording();
    } else if (result.kind === "complete") {
      onChange(result.shortcut);
      setPreviewKeys([]);
      setError("");
      onRecordingChange(false);
    } else {
      setPreviewKeys(result.keys);
      setError(result.kind === "invalid" ? result.message : "");
    }
  };

  const onKeyUp = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!recording || !isModifierCode(event.code)) return;
    event.preventDefault();
    event.stopPropagation();
    setPreviewKeys(modifierDisplayTokens(event));
  };

  return (
    <div className="shortcut-row">
      <span id={`${id}-label`}>{label}</span>
      <div className="shortcut-control">
        <button
          ref={recorderRef}
          type="button"
          className={`shortcut-recorder${recording ? " shortcut-recording" : ""}`}
          aria-labelledby={`${id}-label`}
          aria-pressed={recording}
          onClick={() => {
            setPreviewKeys([]);
            setError("");
            onRecordingChange(true);
          }}
          onBlur={stopRecording}
          onKeyDown={onKeyDown}
          onKeyUp={onKeyUp}
        >
          {keys.length > 0
            ? keys.map((key, index) => <kbd key={`${key}-${index}`}>{key}</kbd>)
            : <span className="shortcut-prompt">Press shortcut…</span>}
        </button>
        {error && <span className="shortcut-error" role="status">{error}</span>}
      </div>
    </div>
  );
}
