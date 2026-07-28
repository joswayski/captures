import { invoke, isTauri } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { message, open } from "@tauri-apps/plugin-dialog";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";

import { formatFileSize } from "./lib/format";
import { reconcileClipboardState } from "./lib/clipboard";
import {
  editorCropAfterDrag,
  formatEditorTime,
  recordingFilenameError,
  recordingFileStem,
  timelineKeyboardDelta,
  type EditorCropHandle,
} from "./lib/recordingEditor";
import {
  dragSelectionRect,
  isCapturableSelection,
  selectionRect,
  type SelectionDragMode,
  type SelectionPoint,
} from "./lib/selection";
import {
  isModifierCode,
  modifierDisplayTokens,
  recordShortcut,
  shortcutDisplayTokens,
} from "./lib/shortcut";
import {
  applyThumbnailNativeHover,
  clearThumbnailNativeHover,
  setThumbnailNativeActiveCard,
  shouldIgnoreThumbnailCursorEvents,
  thumbnailCursorSyncAction,
} from "./lib/thumbnailHover";
import {
  buildThumbnailDustParticles,
  prefersReducedMotion,
  THUMBNAIL_CARD_FALLBACK_HEIGHT,
  THUMBNAIL_CARD_FALLBACK_WIDTH,
  THUMBNAIL_DELETE_ORIGIN_AFTER_CLOSE_X,
  THUMBNAIL_DELETE_ORIGIN_FIRST_X,
  THUMBNAIL_DELETE_ORIGIN_Y,
  type ThumbnailDustParticle,
} from "./lib/thumbnailExit";
import { shouldScrollThumbnailStackToEnd } from "./lib/thumbnailLayout";
import { reconcileActiveViewer } from "./lib/viewerActivation";
import type {
  ActiveSession,
  AudioDevice,
  AppSettings,
  ArtifactDragPayload,
  ArtifactSummary,
  CaptureArtifact,
  CaptureMode,
  ClipboardState,
  EditSpec,
  ExportProgress,
  ExportSpec,
  MaxResolution,
  RecordingArtifact,
  RecordingDraftManifest,
  RecordingOptions,
  RecordingSelectionSession,
  RecordingSessionSnapshot,
  RecordingTarget,
  RecordingTimelinePreview,
  ThumbnailPointerPosition,
  UpdateStatus,
  ViewerActivationState,
} from "./types";

const currentWindow = isTauri() ? getCurrentWindow() : null;
const THUMBNAIL_DISMISS_FALLBACK_MS = 900;
const THUMBNAIL_DELETE_FALLBACK_MS = 3_200;
const RECORDING_SELECTOR_REVEAL_FALLBACK_MS = 200;

function query(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name);
}

function afterNextPaint(callback: () => void) {
  requestAnimationFrame(() => requestAnimationFrame(callback));
}

type SelectOption = {
  value: string;
  label: string;
  description?: string;
  disabled?: boolean;
};

export function CustomSelect({
  value,
  options,
  onChange,
  ariaLabel,
  disabled = false,
  onOpen,
}: {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  ariaLabel: string;
  disabled?: boolean;
  onOpen?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const [menuLayout, setMenuLayout] = useState<{
    placement: "above" | "below";
    maxHeight: number;
  }>({ placement: "below", maxHeight: 240 });
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listboxRef = useRef<HTMLDivElement>(null);
  const listboxId = useId();
  const enabledIndexes = options.flatMap((option, index) => option.disabled ? [] : [index]);
  const selectedIndex = Math.max(0, options.findIndex((option) => option.value === value));
  const selected = options[selectedIndex] ?? options[0];
  const activeOptionId = `${listboxId}-option-${activeIndex}`;

  const openMenu = () => {
    if (disabled) return;
    onOpen?.();
    setActiveIndex(options[selectedIndex]?.disabled ? (enabledIndexes[0] ?? 0) : selectedIndex);
    setOpen(true);
  };
  const closeMenu = () => setOpen(false);
  const choose = (index: number) => {
    const option = options[index];
    if (!option || option.disabled) return;
    onChange(option.value);
    closeMenu();
    requestAnimationFrame(() => triggerRef.current?.focus());
  };
  const moveActive = (direction: 1 | -1) => {
    if (enabledIndexes.length === 0) return;
    const current = enabledIndexes.indexOf(activeIndex);
    const next = current < 0
      ? (direction === 1 ? 0 : enabledIndexes.length - 1)
      : (current + direction + enabledIndexes.length) % enabledIndexes.length;
    setActiveIndex(enabledIndexes[next]);
  };

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) closeMenu();
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    return () => window.removeEventListener("pointerdown", onPointerDown, true);
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current || !listboxRef.current) return;
    const triggerBounds = triggerRef.current.getBoundingClientRect();
    const measuredHeight = listboxRef.current.scrollHeight
      || Math.min(240, options.length * 31 + 8);
    const desiredHeight = Math.min(240, measuredHeight);
    const spaceAbove = Math.max(0, triggerBounds.top - 8);
    const spaceBelow = Math.max(0, window.innerHeight - triggerBounds.bottom - 8);
    const placement = spaceBelow < desiredHeight && spaceAbove > spaceBelow ? "above" : "below";
    const availableHeight = placement === "above" ? spaceAbove : spaceBelow;
    const nextLayout = {
      placement,
      maxHeight: Math.max(72, Math.min(240, availableHeight - 5)),
    } as const;
    setMenuLayout((current) => (
      current.placement === nextLayout.placement && current.maxHeight === nextLayout.maxHeight
        ? current
        : nextLayout
    ));
  }, [open, options.length]);

  return (
    <div
      className={`custom-select${open ? " open" : ""}${open && menuLayout.placement === "above" ? " open-above" : ""}`}
      ref={rootRef}
    >
      <button
        ref={triggerRef}
        type="button"
        className="custom-select-trigger"
        role="combobox"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listboxId : undefined}
        aria-activedescendant={open ? activeOptionId : undefined}
        disabled={disabled}
        onClick={() => open ? closeMenu() : openMenu()}
        onBlur={(event) => {
          if (!rootRef.current?.contains(event.relatedTarget as Node | null)) closeMenu();
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape" && open) {
            event.preventDefault();
            closeMenu();
          } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            if (!open) openMenu();
            else moveActive(event.key === "ArrowDown" ? 1 : -1);
          } else if (event.key === "Home" && open) {
            event.preventDefault();
            setActiveIndex(enabledIndexes[0] ?? 0);
          } else if (event.key === "End" && open) {
            event.preventDefault();
            setActiveIndex(enabledIndexes.at(-1) ?? 0);
          } else if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            if (open) choose(activeIndex);
            else openMenu();
          }
        }}
      >
        <span>{selected?.label ?? value}</span>
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 6 4 4 4-4" /></svg>
      </button>
      {open && (
        <div
          ref={listboxRef}
          id={listboxId}
          className="custom-select-listbox"
          role="listbox"
          aria-label={ariaLabel}
          style={{ maxHeight: menuLayout.maxHeight }}
        >
          {options.map((option, index) => (
            <button
              key={`${option.value}-${index}`}
              id={`${listboxId}-option-${index}`}
              type="button"
              role="option"
              aria-selected={option.value === value}
              disabled={option.disabled}
              className={activeIndex === index ? "active" : ""}
              onPointerEnter={() => {
                if (!option.disabled) setActiveIndex(index);
              }}
              onClick={() => choose(index)}
            >
              <span className="custom-select-option-copy">
                <span>{option.label}</span>
                {option.description && <small>{option.description}</small>}
              </span>
              {option.value === value && <span aria-hidden="true">✓</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function emitViewerActivation(artifactId: string | null, active: boolean) {
  if (!artifactId) return;
  void emit<ViewerActivationState>("viewer-activation-changed", {
    artifact_id: artifactId,
    active,
  }).catch(() => undefined);
}

export function App() {
  const view = query("view");
  if (view === "overlay") return <CaptureOverlay />;
  if (view === "recording-selector") return <RecordingSelector />;
  if (view === "recording-countdown") return <RecordingCountdown />;
  if (view === "recording-hud") return <RecordingHud />;
  if (view === "recording-editor") return <RecordingEditor />;
  if (view === "thumbnail") return <Thumbnail />;
  if (view === "viewer") return <ArtifactViewer />;
  if (view === "history") return <CaptureHistory />;
  if (view === "preferences") return <Preferences />;
  if (view === "startup") return <StartupNotice />;
  if (view === "update") return <UpdateNotice />;
  return <IdleView />;
}

function IdleView() {
  return (
    <main className="idle-view">
      <div className="brand-mark">Captures</div>
      <h1>Captures is running</h1>
      <p>Use the capture shortcut or the tray icon to take a screenshot.</p>
    </main>
  );
}

function StartupNotice() {
  return (
    <main className="startup-notice">
      <div className="startup-icon" aria-hidden="true">
        <svg viewBox="0 0 24 24">
          <path d="M9 4H7a3 3 0 0 0-3 3v2M15 4h2a3 3 0 0 1 3 3v2M20 15v2a3 3 0 0 1-3 3h-2M9 20H7a3 3 0 0 1-3-3v-2" />
          <path className="startup-icon-spark" d="M12 8.5c.4 1.8 1.7 3.1 3.5 3.5-1.8.4-3.1 1.7-3.5 3.5-.4-1.8-1.7-3.1-3.5-3.5 1.8-.4 3.1-1.7 3.5-3.5Z" />
        </svg>
      </div>
      <div>
        <strong>Captures is running</strong>
        <p>Use the tray icon or Ctrl+Shift+4 to capture.</p>
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

export function UpdateNotice() {
  const status = useUpdateStatus();
  const [actionError, setActionError] = useState("");

  const close = () => {
    setActionError("");
    void currentWindow?.hide();
  };
  const run = async (command: "check_for_updates" | "install_update") => {
    setActionError("");
    try {
      await invoke(command);
    } catch (error) {
      setActionError(String(error));
    }
  };

  const available = status?.state === "available" ? status : null;
  const downloading = status?.state === "downloading" ? status : null;
  const error = actionError || (status?.state === "error" ? status.message : "");
  const progress = downloading?.total
    ? Math.min(100, Math.round((downloading.downloaded / downloading.total) * 100))
    : null;

  return (
    <main className="update-notice">
      <div className="update-notice-header">
        <div className="update-icon" aria-hidden="true">↓</div>
        <div>
          <span className="eyebrow">Captures update</span>
          <strong>
            {available || downloading
              ? `Version ${(available ?? downloading)!.display_version} is available`
              : status?.state === "up_to_date"
                ? "Captures is up to date"
                : status?.state === "checking"
                  ? "Checking for updates…"
                  : error
                    ? "The update could not be installed"
                    : "Preparing update information…"}
          </strong>
        </div>
      </div>

      {available?.notes && <p className="update-notes">{available.notes}</p>}
      {downloading && (
        <div className="update-progress" role="progressbar" aria-valuenow={progress ?? undefined}>
          <span style={{ width: `${progress ?? 15}%` }} />
          <small>{progress === null ? "Downloading update…" : `Downloading… ${progress}%`}</small>
        </div>
      )}
      {error && <p className="update-error" role="alert">{error}</p>}

      <div className="update-actions">
        {available && (
          <button className="primary" type="button" onClick={() => void run("install_update")}>
            {available.installable ? "Install & Restart" : "Download Release"}
          </button>
        )}
        {error && (
          <button className="primary" type="button" onClick={() => void run("check_for_updates")}>Try Again</button>
        )}
        {!available && !downloading && !error && status?.state !== "checking" && (
          <button className="primary" type="button" onClick={() => void run("check_for_updates")}>Check Again</button>
        )}
        <button type="button" onClick={close} disabled={Boolean(downloading)}>Later</button>
      </div>
    </main>
  );
}

function UpdatePreferences() {
  const status = useUpdateStatus();
  const [actionError, setActionError] = useState("");
  const currentVersion = status?.current_display_version ?? "…";
  const available = status?.state === "available" ? status : null;
  const downloading = status?.state === "downloading";

  const run = async (command: "check_for_updates" | "install_update") => {
    setActionError("");
    try {
      await invoke(command);
    } catch (error) {
      setActionError(String(error));
    }
  };

  return (
    <section className="settings-section update-settings">
      <h2>Updates</h2>
      <div className="update-settings-row">
        <div>
          <strong>Version {currentVersion}</strong>
          <small>
            {available
              ? `Version ${available.display_version} is available.`
              : status?.state === "up_to_date"
                ? "Captures is up to date."
                : status?.state === "checking"
                  ? "Checking GitHub Releases…"
                  : status?.state === "error"
                    ? status.message
                    : "Captures checks GitHub Releases for signed updates."}
          </small>
        </div>
        <button
          type="button"
          disabled={status?.state === "checking" || downloading}
          onClick={() => void run(available ? "install_update" : "check_for_updates")}
        >
          {downloading
            ? "Installing…"
            : available
              ? available.installable ? "Install & Restart" : "Download Release"
              : status?.state === "checking" ? "Checking…" : "Check Now"}
        </button>
      </div>
      {actionError && <p className="update-settings-error" role="alert">{actionError}</p>}
    </section>
  );
}

function ArtifactViewer() {
  const artifactId = query("artifact_id");
  const [artifact, setArtifact] = useState<CaptureArtifact | null>(null);
  const [fit, setFit] = useState(true);

  useEffect(() => {
    let active = true;
    let dispose: (() => void)[] = [];
    void (async () => {
      dispose = await Promise.all([
        listen<string>("artifact-removed", ({ payload }) => {
          if (artifactId !== payload) return;
          emitViewerActivation(artifactId, false);
          void currentWindow?.close();
        }),
      ]);
      if (!artifactId) return;
      const initialArtifact = await invoke<CaptureArtifact | null>("get_artifact", { artifactId });
      if (active) {
        setArtifact(initialArtifact);
      }
    })();
    return () => {
      active = false;
      dispose.forEach((unlisten) => unlisten());
    };
  }, [artifactId]);

  useEffect(() => {
    if (!currentWindow) return;
    let active = true;
    let dispose: (() => void)[] = [];

    void (async () => {
      const stopListening = await Promise.all([
        currentWindow.onFocusChanged(({ payload }) => {
          if (active && payload) emitViewerActivation(artifactId, true);
        }),
        currentWindow.onCloseRequested(() => {
          if (active) emitViewerActivation(artifactId, false);
        }),
      ]);
      if (!active) {
        stopListening.forEach((unlisten) => unlisten());
        return;
      }
      dispose = stopListening;
      const focused = await currentWindow.isFocused();
      if (active && focused) emitViewerActivation(artifactId, true);
    })();

    return () => {
      active = false;
      dispose.forEach((unlisten) => unlisten());
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
        <button type="button" onClick={() => setFit((current) => !current)}>
          {fit ? "Actual size" : "Fit to window"}
        </button>
      </header>
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

export function CaptureHistory() {
  const [entries, setEntries] = useState<ArtifactSummary[]>([]);
  const [drafts, setDrafts] = useState<RecordingDraftManifest[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const activeRef = useRef(true);

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
    let dispose: (() => void) | undefined;
    activeRef.current = true;

    void (async () => {
      dispose = await listen("capture-history-changed", () => {
        void refresh();
      });
      await refresh();
    })();

    return () => {
      activeRef.current = false;
      dispose?.();
    };
  }, [refresh]);

  return (
    <main className="capture-history">
      <header className="history-header">
        <div>
          <p className="eyebrow">ON THIS MAC</p>
          <h1>Capture History</h1>
          <p>Screenshots, videos, GIFs, and interrupted recordings you can recover all appear here.</p>
        </div>
        {!loading && entries.length > 0 && (
          <span className="history-count">
            {entries.length} {entries.length === 1 ? "capture" : "captures"}
          </span>
        )}
      </header>

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
      ) : entries.length > 0 ? (
        <section className="history-grid" aria-label="Recent captures">
          {entries.map((entry) => (
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
  const [busy, setBusy] = useState<"restoring" | "opening" | "revealing" | "deleting" | null>(null);
  const [restored, setRestored] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [error, setError] = useState("");
  const feedbackTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const deleteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

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

  const deleteFromHistory = async () => {
    if (busy) return;
    if (!confirmingDelete) {
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

  return (
    <article className="history-card">
      <div className={`history-image-wrap${entry.kind !== "screenshot" && entry.missing ? " history-image-missing" : ""}`}>
        <img
          src={entry.kind === "screenshot" ? entry.preview_url : entry.poster_url}
          alt={entry.kind === "screenshot" ? "Screenshot from capture history" : `${entry.kind === "gif" ? "GIF" : "Video"} recording poster`}
          loading="lazy"
          draggable={false}
        />
        <span className="history-mode">{entry.kind === "screenshot" ? formatCaptureMode(entry.mode) : entry.kind.toUpperCase()}</span>
        {entry.kind !== "screenshot" && entry.missing && <span className="history-missing-label">File missing</span>}
      </div>
      <div className="history-card-body">
        <time dateTime={entry.created_at}>{formatHistoryDate(entry.created_at)}</time>
        <p>
          {entry.width} × {entry.height} · {formatFileSize(entry.size_bytes)}
          {entry.kind !== "screenshot" && <> · {formatRecordingTime(entry.duration_ms)}</>}
        </p>
        {entry.kind !== "screenshot" && entry.dropped_frames > 0 && <p className="history-recording-warning">{entry.dropped_frames.toLocaleString()} frame{entry.dropped_frames === 1 ? "" : "s"} dropped while recording</p>}
        <div className={`history-actions${entry.kind === "screenshot" ? "" : " history-recording-actions"}`}>
          {entry.kind === "screenshot" ? (
            <button
              type="button"
              className="history-restore"
              disabled={busy !== null}
              onClick={() => void restore()}
            >
              {restored ? <><CheckIcon />Restored</> : <><RestoreIcon />{busy === "restoring" ? "Restoring…" : "Restore"}</>}
            </button>
          ) : (
            <>
              <button
                type="button"
                className="history-restore"
                disabled={busy !== null || entry.missing}
                onClick={() => void openRecording()}
              >
                {busy === "opening" ? "Opening…" : entry.missing ? "File missing" : "Edit"}
              </button>
              <button
                type="button"
                className="history-reveal"
                disabled={busy !== null || entry.missing}
                onClick={() => void revealRecording()}
              >
                {busy === "revealing" ? "Showing…" : "Show in Folder"}
              </button>
            </>
          )}
          <button
            type="button"
            className={confirmingDelete ? "history-delete history-delete-confirm" : "history-delete"}
            aria-label={confirmingDelete
              ? entry.kind === "screenshot" ? "Confirm permanent deletion" : "Confirm removal from History"
              : entry.kind === "screenshot" ? "Delete from History" : "Remove from History"}
            disabled={busy !== null}
            onClick={() => void deleteFromHistory()}
          >
            <TrashIcon />
            {confirmingDelete ? (entry.kind === "screenshot" ? "Delete forever" : "Remove entry") : (entry.kind === "screenshot" ? "Delete" : "Remove")}
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

function formatCaptureMode(mode: CaptureMode): string {
  if (mode === "display") return "Full screen";
  return mode[0].toUpperCase() + mode.slice(1);
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

function CameraIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 8h3l1.5-2h7L17 8h3v11H4Z" /><circle cx="12" cy="13.5" r="3.5" /></svg>;
}

function MicrophoneIcon({ muted }: { muted: boolean }) {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="9" y="3" width="6" height="11" rx="3" /><path d="M6 11a6 6 0 0 0 11.4 2.6M12 18v3M9 21h6" />{muted && <path d="m4 4 16 16" />}</svg>;
}

function HiddenFromCaptureIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 12s3.5-5 9-5 9 5 9 5-3.5 5-9 5-9-5-9-5Z" /><path d="m4 4 16 16" /></svg>;
}

function DragGripIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 7h8M8 12h8M8 17h8" /></svg>;
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

export function RecordingCountdown() {
  const [snapshot, setSnapshot] = useState<RecordingSessionSnapshot | null>(null);
  const [remaining, setRemaining] = useState<number | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const cancellingRef = useRef(false);

  useEffect(() => {
    let active = true;
    const dispose: (() => void)[] = [];
    void Promise.all([
      listen<RecordingSessionSnapshot>("recording-state-changed", ({ payload }) => {
        if (!active) return;
        setSnapshot(payload);
        if (payload.state !== "countdown") setRemaining(null);
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
      if (active && current) setSnapshot(current);
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
    try {
      await invoke("discard_recording", { sessionId: snapshot.id });
    } finally {
      cancellingRef.current = false;
      setCancelling(false);
    }
  }, [snapshot]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      void cancel();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [cancel]);

  const count = remaining ?? snapshot?.countdown_remaining_seconds ?? snapshot?.options.countdown_seconds ?? 3;
  return (
    <main className="recording-countdown" aria-live="assertive">
      <div className="recording-countdown-content">
        <span>Recording starts in</span>
        <strong>{count}</strong>
        <div className="recording-countdown-steps" aria-hidden="true">
          {[3, 2, 1].map((step) => <i key={step} className={step === count ? "active" : ""}>{step}</i>)}
        </div>
        <small>{cancelling ? "Cancelling…" : "Press Esc to cancel"}</small>
      </div>
    </main>
  );
}

export function RecordingSelector() {
  const [session, setSession] = useState<RecordingSelectionSession | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [devicesLoading, setDevicesLoading] = useState(false);
  const [devicesLoaded, setDevicesLoaded] = useState(false);
  const [targetMode, setTargetMode] = useState<RecordingTargetMode>("region");
  const [region, setRegion] = useState<RecordingRect | null>(null);
  const [panelPosition, setPanelPosition] = useState<RecordingPanelPosition | null>(null);
  const [panelDragging, setPanelDragging] = useState(false);
  const [selectedWindow, setSelectedWindow] = useState<string | null>(null);
  const [hoveredWindow, setHoveredWindow] = useState<string | null>(null);
  const [fps, setFps] = useState(60);
  const [maxResolution, setMaxResolution] = useState<MaxResolution>("original");
  const [showCursor, setShowCursor] = useState(true);
  const [systemAudio, setSystemAudio] = useState(false);
  const [microphoneId, setMicrophoneId] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState("");
  const surfaceRef = useRef<HTMLElement>(null);
  const panelRef = useRef<HTMLElement>(null);
  const panelDragRef = useRef<RecordingPanelDrag | null>(null);
  const regionDragRef = useRef<RecordingRegionDrag | null>(null);
  const pendingRegionPointRef = useRef<SelectionPoint | null>(null);
  const regionFrameRef = useRef<number | null>(null);
  const settingsRef = useRef<AppSettings | null>(null);
  const activeSessionIdRef = useRef<string | null>(null);
  const revealingSessionIdRef = useRef<string | null>(null);

  const clearRegionDrag = useCallback(() => {
    if (regionFrameRef.current !== null) {
      window.cancelAnimationFrame(regionFrameRef.current);
      regionFrameRef.current = null;
    }
    regionDragRef.current = null;
    pendingRegionPointRef.current = null;
  }, []);

  const loadAudioDevices = useCallback(() => {
    if (devicesLoading || devicesLoaded) return;
    setDevicesLoading(true);
    void invoke<AudioDevice[]>("list_recording_audio_devices")
      .then((audioDevices) => {
        setDevices(audioDevices);
        setDevicesLoaded(true);
      })
      .catch(() => {
        setDevices([]);
        setDevicesLoaded(true);
      })
      .finally(() => setDevicesLoading(false));
  }, [devicesLoaded, devicesLoading]);

  const revealSelector = useCallback((selectionId: string) => {
    if (activeSessionIdRef.current !== selectionId) return;
    if (revealingSessionIdRef.current === selectionId) return;
    revealingSessionIdRef.current = selectionId;
    void invoke("show_recording_selector", { selectionId }).then(() => {
      let revealStarted = false;
      const finishReveal = () => {
        if (revealStarted || activeSessionIdRef.current !== selectionId) return;
        revealStarted = true;
        window.clearTimeout(fallbackTimer);
        void invoke("reveal_recording_selector", { selectionId }).catch((error) => {
          if (revealingSessionIdRef.current === selectionId) {
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
      if (revealingSessionIdRef.current === selectionId) {
        revealingSessionIdRef.current = null;
        setError(String(error));
      }
    });
  }, []);

  const cancelSelection = useCallback((selection: RecordingSelectionSession) => {
    if (activeSessionIdRef.current !== selection.id) return;
    activeSessionIdRef.current = null;
    revealingSessionIdRef.current = null;
    setSession(null);
    clearRegionDrag();
    panelDragRef.current = null;
    setPanelDragging(false);
    setStarting(false);
    setError("");
    void invoke("cancel_recording_selection", { selectionId: selection.id }).catch((error) => {
      // A new selector may already be active by the time a stale cancellation
      // fails. Never replace that newer session with the one being dismissed.
      if (activeSessionIdRef.current !== null) return;
      activeSessionIdRef.current = selection.id;
      setSession(selection);
      setError(String(error));
      revealSelector(selection.id);
    });
  }, [clearRegionDrag, revealSelector]);

  useEffect(() => {
    let active = true;
    let dispose: (() => void) | undefined;
    const applySelection = (selection: RecordingSelectionSession, currentSettings: AppSettings) => {
      activeSessionIdRef.current = selection.id;
      revealingSessionIdRef.current = null;
      setSession(selection);
      setFps(currentSettings.recording.video_fps);
      setMaxResolution(currentSettings.recording.video_max_resolution);
      setShowCursor(currentSettings.recording.show_cursor);
      setSystemAudio(currentSettings.recording.capture_system_audio);
      setMicrophoneId(currentSettings.recording.microphone_device_id);
      setTargetMode("region");
      setSelectedWindow(null);
      setHoveredWindow(null);
      setRegion(defaultRecordingRegion(selection.display.width, selection.display.height));
      clearRegionDrag();
      panelDragRef.current = null;
      setPanelDragging(false);
      setPanelPosition(null);
      setStarting(false);
      setError("");
    };
    const onSelectionReady = ({ payload }: { payload: RecordingSelectionSession }) => {
      if (!active) return;
      const currentSettings = settingsRef.current;
      void invoke<AppSettings>("get_settings").then((latestSettings) => {
        if (!active) return;
        settingsRef.current = latestSettings;
        setSettings(latestSettings);
        applySelection(payload, latestSettings);
      }).catch(() => {
        if (!active) return;
        if (currentSettings) {
          applySelection(payload, currentSettings);
        } else {
          void invoke("cancel_recording_selection", { selectionId: payload.id });
        }
      });
    };

    // Register for future selections, but do not wait for the Tauri event
    // bridge before loading the selection already prepared by the backend.
    // A newly-created macOS WebView can otherwise sit on a blank, click-through
    // surface while event registration is still pending.
    void listen<RecordingSelectionSession>("recording-selection-ready", onSelectionReady)
      .then((unlisten) => {
        if (active) {
          dispose = unlisten;
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
    ])
      .then(([pending, loadedSettings]) => {
        if (!active) return;
        settingsRef.current = loadedSettings;
        setSettings(loadedSettings);
        if (pending) {
          applySelection(pending, loadedSettings);
        }
      })
      .catch((error) => {
        if (active) setError(String(error));
      });
    return () => {
      active = false;
      activeSessionIdRef.current = null;
      clearRegionDrag();
      dispose?.();
    };
  }, [clearRegionDrag]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !session) return;
      event.preventDefault();
      cancelSelection(session);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [cancelSelection, session]);

  // A hidden WKWebView can defer image loading until its native window is
  // onscreen. Do not make the selector depend exclusively on the snapshot's
  // onLoad event or the window can remain hidden forever. The native surface is
  // transparent and click-through while this safety path waits for paint.
  useEffect(() => {
    if (!session?.id) return;
    const timer = window.setTimeout(() => {
      revealSelector(session.id);
    }, 120);
    return () => window.clearTimeout(timer);
  }, [session?.id, revealSelector]);

  useEffect(() => {
    if (!session?.id) return;
    const timer = window.setTimeout(loadAudioDevices, 0);
    return () => window.clearTimeout(timer);
  }, [loadAudioDevices, session?.id]);

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
  const onPointerDown = (event: React.PointerEvent) => {
    if (targetMode !== "region" || (event.target as Element).closest(".recording-selector-panel")) return;
    event.preventDefault();
    const start = point(event);
    const target = event.target as Element;
    const handle = target.closest<HTMLElement>("[data-selection-handle]")?.dataset.selectionHandle as SelectionDragMode | undefined;
    const mode: SelectionDragMode = handle
      ?? (target.closest(".recording-selection-frame") ? "move" : "create");
    if (mode !== "create" && !region) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    clearRegionDrag();
    regionDragRef.current = {
      mode,
      origin: start,
      initial: region ?? { x: start.x, y: start.y, width: 0, height: 0 },
    };
    if (mode === "create") setRegion({ x: start.x, y: start.y, width: 0, height: 0 });
  };
  const onPointerMove = (event: React.PointerEvent) => {
    if (!regionDragRef.current || targetMode !== "region") return;
    event.preventDefault();
    pendingRegionPointRef.current = point(event);
    if (regionFrameRef.current !== null) return;
    regionFrameRef.current = window.requestAnimationFrame(() => {
      regionFrameRef.current = null;
      const drag = regionDragRef.current;
      const current = pendingRegionPointRef.current;
      if (!drag || !current) return;
      setRegion(dragSelectionRect(
        drag.mode,
        drag.origin,
        current,
        drag.initial,
        { width: session.display.width, height: session.display.height },
      ));
    });
  };
  const onPointerUp = (event: React.PointerEvent) => {
    const drag = regionDragRef.current;
    const current = pendingRegionPointRef.current;
    if (drag && current) {
      setRegion(dragSelectionRect(
        drag.mode,
        drag.origin,
        current,
        drag.initial,
        { width: session.display.width, height: session.display.height },
      ));
    }
    if (
      typeof event.currentTarget.hasPointerCapture === "function"
      && event.currentTarget.hasPointerCapture(event.pointerId)
    ) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    clearRegionDrag();
  };

  const selectableWindows = session.windows.filter((window) => !isCapturesOwnedWindow(window));
  const windowLayouts = selectableWindows.map((window, index) => {
    const scale = Math.max(session.window_coordinate_scale || 1, 1);
    return {
      window,
      left: (window.x - session.display.x) / scale,
      top: (window.y - session.display.y) / scale,
      width: window.width / scale,
      height: window.height / scale,
      zIndex: selectableWindows.length - index,
    };
  });
  const activeWindow = hoveredWindow ?? selectedWindow;
  const activeWindowLayout = windowLayouts.find(({ window }) => window.id === activeWindow);
  const selectedRect = targetMode === "display"
    ? { x: 0, y: 0, width: session.display.width, height: session.display.height }
    : targetMode === "window"
      ? activeWindowLayout ? {
          x: activeWindowLayout.left,
          y: activeWindowLayout.top,
          width: activeWindowLayout.width,
          height: activeWindowLayout.height,
        } : null
      : region;
  const canStart = targetMode === "display"
    || (targetMode === "window" && Boolean(selectedWindow))
    || (targetMode === "region" && Boolean(region && region.width >= 16 && region.height >= 16));

  const start = async () => {
    if (!canStart || starting) return;
    let target: RecordingTarget;
    if (targetMode === "display") {
      target = { type: "display", display_id: session.display.id };
    } else if (targetMode === "window" && selectedWindow) {
      target = { type: "window", window_id: selectedWindow };
    } else if (region) {
      target = {
        type: "region",
        display_id: session.display.id,
        rect: roundRecordingRect(region, session.display.width, session.display.height),
      };
    } else {
      return;
    }
    const options: RecordingOptions = {
      kind: "video",
      target,
      frames_per_second: fps,
      max_resolution: maxResolution,
      countdown_seconds: settings.recording.countdown_seconds,
      show_cursor: showCursor,
      highlight_clicks: settings.recording.highlight_clicks,
      show_keystrokes: settings.recording.show_keystrokes,
      audio: {
        capture_system_audio: systemAudio,
        microphone_device_id: microphoneId,
        mono_output: settings.recording.mono_audio,
        system_volume_percent: 100,
        microphone_volume_percent: 100,
        microphone_muted: false,
      },
      gif: {
        max_width: settings.recording.gif_max_width,
        max_colors: settings.recording.gif_max_colors,
        optimize: true,
      },
    };
    setStarting(true);
    setError("");
    try {
      await invoke("start_recording", {
        request: { selection_id: session.id, options },
      });
      activeSessionIdRef.current = null;
      revealingSessionIdRef.current = null;
      setSession(null);
      clearRegionDrag();
    } catch (error) {
      setError(String(error));
      setStarting(false);
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

  return (
    <main
      ref={surfaceRef}
      className={`recording-selector recording-target-${targetMode}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDragStart={(event) => event.preventDefault()}
    >
      <img
        className="recording-selector-snapshot"
        src={session.snapshot_url}
        alt=""
        draggable={false}
        onLoad={() => revealSelector(session.id)}
        onError={() => {
          setError("The frozen preview could not load. You can still select from the live desktop.");
          revealSelector(session.id);
        }}
      />
      <CaptureDim
        mode={targetMode}
        hole={selectedRect}
        bounds={{ width: session.display.width, height: session.display.height }}
      />
      {targetMode !== "window" && selectedRect && selectedRect.width > 0 && selectedRect.height > 0 && (
        <div
          className={`recording-selection-frame recording-selection-${targetMode}${targetMode === "region" ? " movable" : ""}`}
          style={{
            left: selectedRect.x,
            top: selectedRect.y,
            width: selectedRect.width,
            height: selectedRect.height,
          }}
        >
          <span>{Math.round(selectedRect.width)} × {Math.round(selectedRect.height)}</span>
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
          {windowLayouts.map(({ window, ...layout }) => (
            <button
              key={window.id}
              type="button"
              className={`recording-window-target${selectedWindow === window.id ? " selected" : ""}${hoveredWindow === window.id ? " hovered" : ""}`}
              style={layout}
              aria-label={`Select ${window.title || "window"}`}
              onPointerDown={(event) => event.stopPropagation()}
              onMouseEnter={() => setHoveredWindow(window.id)}
              onMouseLeave={() => setHoveredWindow(null)}
              onClick={() => setSelectedWindow(window.id)}
            >
              <span>{window.title || window.app_name || "Window"}</span>
            </button>
          ))}
        </div>
      )}

      <section
        ref={panelRef}
        className={`recording-selector-panel${panelDragging ? " dragging" : ""}`}
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
          <div className="recording-target-switch" role="group" aria-label="Capture target">
            {(["region", "window", "display"] as const).map((mode) => (
              <button
                key={mode}
                type="button"
                className={targetMode === mode ? "active" : ""}
                onClick={() => {
                  setTargetMode(mode);
                  setHoveredWindow(null);
                  if (mode === "window") {
                    setSelectedWindow((current) => (
                      current && selectableWindows.some((window) => window.id === current)
                        ? current
                        : selectableWindows[0]?.id ?? null
                    ));
                  }
                }}
              >
                {mode === "display" ? "Full screen" : mode[0].toUpperCase() + mode.slice(1)}
              </button>
            ))}
          </div>
          <button className="recording-cancel" type="button" onClick={() => cancelSelection(session)}>Cancel</button>
        </div>
        {targetMode === "window" && (
          <p className="recording-selection-guidance">
            {selectedWindow
              ? "Window selected. Hover another window to preview it, then click to choose."
              : "Select a window to get started."}
          </p>
        )}
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
          <div className="recording-field"><span>Cursor</span>
            <label className="recording-toggle">
              <input aria-label="Show cursor" type="checkbox" checked={showCursor} onChange={(event) => setShowCursor(event.target.checked)} />
              <span className="recording-switch" aria-hidden="true" />
              <span>{showCursor ? "On" : "Off"}</span>
            </label>
          </div>
          <div className="recording-field"><span>Desktop audio</span>
            <label className="recording-toggle">
              <input aria-label="Record desktop audio" type="checkbox" checked={systemAudio} onChange={(event) => setSystemAudio(event.target.checked)} />
              <span className="recording-switch" aria-hidden="true" />
              <span>{systemAudio ? "On" : "Off"}</span>
            </label>
          </div>
          <div className="recording-field recording-microphone-field"><span>Microphone</span>
            <CustomSelect
              value={microphoneId ?? "off"}
              disabled={devicesLoading}
              onOpen={loadAudioDevices}
              ariaLabel="Microphone"
              options={[
                { value: "off", label: "Off" },
                ...(devicesLoading ? [{ value: "__loading", label: "Loading microphones…", disabled: true }] : []),
                ...(microphoneId && !devices.some((device) => device.id === microphoneId)
                  ? [{ value: microphoneId, label: devicesLoading ? "Loading microphone…" : "Selected microphone" }]
                  : []),
                ...devices.map((device) => ({ value: device.id, label: device.name })),
              ]}
              onChange={(value) => setMicrophoneId(value === "off" ? null : value)}
            />
          </div>
          <div className="recording-field recording-action-field"><span>Ready</span>
            <button className="recording-start" type="button" disabled={!canStart || starting} onClick={() => void start()}>
              <span aria-hidden="true" />{starting ? "Starting…" : "Record"}
            </button>
          </div>
        </div>
        {error && <p className="recording-selector-error" role="alert">{error}</p>}
      </section>
    </main>
  );
}

function isCapturesOwnedWindow(window: RecordingSelectionSession["windows"][number]): boolean {
  const owner = window.app_name?.trim().replace(/\.app$/i, "").toLowerCase();
  return owner === "captures";
}

function defaultRecordingRegion(width: number, height: number): RecordingRect {
  const regionWidth = Math.max(320, Math.round(width * 0.66));
  const regionHeight = Math.max(240, Math.round(height * 0.62));
  return {
    x: Math.round((width - regionWidth) / 2),
    y: Math.round((height - regionHeight) / 2),
    width: Math.min(width, regionWidth),
    height: Math.min(height, regionHeight),
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

export function RecordingHud() {
  const [snapshot, setSnapshot] = useState<RecordingSessionSnapshot | null>(null);
  const [busy, setBusy] = useState(false);
  const [microphonePeak, setMicrophonePeak] = useState(0);
  const [error, setError] = useState("");
  const sessionIdRef = useRef<string | null>(null);

  useEffect(() => {
    let active = true;
    const dispose: (() => void)[] = [];
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
          if (active && payload.session_id === sessionIdRef.current) setError(payload.message);
        }),
        listen<{ session_id: string; microphone_peak: number }>("recording-audio-level", ({ payload }) => {
          if (active && payload.session_id === sessionIdRef.current) {
            setMicrophonePeak(Math.max(0, Math.min(1, payload.microphone_peak)));
          }
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
      setError(String(error));
    } finally {
      setBusy(false);
    }
  };
  const takeScreenshot = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await invoke("start_capture", { mode: "region" });
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  };
  const startHudDrag = (event: React.PointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0 || !currentWindow) return;
    event.preventDefault();
    void currentWindow.startDragging().catch((error) => setError(String(error)));
  };
  const canControl = snapshot.state === "recording" || snapshot.state === "paused";
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

  return (
    <main className={`recording-hud recording-hud-${snapshot.state}`}>
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
        <HudTooltip label="Restart recording">
          <button type="button" className="recording-icon-button" disabled={!canControl || busy} aria-label="Restart recording" onClick={() => void invokeAction("restart_recording")}><RestartRecordingIcon /></button>
        </HudTooltip>
        <HudTooltip label="Take a region screenshot">
          <button type="button" className="recording-icon-button" disabled={!canControl || busy} aria-label="Take a region screenshot" onClick={() => void takeScreenshot()}><CameraIcon /></button>
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
        <span className="recording-hud-privacy" aria-label="Not included in this recording">
          <HiddenFromCaptureIcon /><span>Not in recording</span>
        </span>
        <HudTooltip label="Drag to move controls">
          <button type="button" className="recording-icon-button recording-drag" aria-label="Move recording controls" onPointerDown={startHudDrag}><DragGripIcon /></button>
        </HudTooltip>
      </div>
      {(error || snapshot.error) && <p className="recording-hud-error" role="alert">{error || snapshot.error}</p>}
    </main>
  );
}

function recordingStatusLabel(snapshot: RecordingSessionSnapshot): string {
  if (snapshot.state === "countdown") return "Starting…";
  if (snapshot.state === "paused") return "Paused";
  if (snapshot.state === "finalizing") return "Saving…";
  if (snapshot.state === "failed") return "Failed";
  return snapshot.options.kind.toUpperCase();
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

type EditorCropDrag = {
  handle: EditorCropHandle;
  start: SelectionPoint;
  initial: RecordingRect;
};

type FileSizeUnit = "kb" | "mb" | "gb";

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
  const [outputFormat, setOutputFormat] = useState<"mp4" | "gif">("mp4");
  const [gifFps, setGifFps] = useState(15);
  const [gifMaxWidth, setGifMaxWidth] = useState(800);
  const [gifColors, setGifColors] = useState(256);
  const [quality, setQuality] = useState<"high" | "standard" | "small">("high");
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
  const [filenameStem, setFilenameStem] = useState("");
  const [destinationDirectory, setDestinationDirectory] = useState("");
  const [previewPlaying, setPreviewPlaying] = useState(false);
  const [toast, setToast] = useState("");
  const [error, setError] = useState("");
  const videoRef = useRef<HTMLVideoElement>(null);
  const previewMediaRef = useRef<HTMLDivElement>(null);
  const timelineRef = useRef<HTMLDivElement>(null);
  const timelineScrubbingRef = useRef(false);
  const trimDragRef = useRef<"start" | "end" | null>(null);
  const cropDragRef = useRef<EditorCropDrag | null>(null);

  useEffect(() => {
    let active = true;
    let dispose: (() => void)[] = [];
    void (async () => {
      dispose = await Promise.all([
        listen<{ export_id: string; progress: ExportProgress }>("recording-export-progress", ({ payload }) => {
          if (active && payload.export_id === exportIdRef.current) setProgress(payload.progress);
        }),
        listen<{ export_id: string; artifact: RecordingArtifact; finder_error: string | null }>("recording-export-complete", ({ payload }) => {
          if (!active || payload.export_id !== exportIdRef.current) return;
          setExported(payload.artifact);
          setToast(
            `Saved ${formatFileSize(payload.artifact.size_bytes)} (${payload.artifact.size_bytes.toLocaleString()} bytes).`,
          );
          if (payload.finder_error) {
            setError(`The recording was saved, but Finder could not open: ${payload.finder_error}`);
          }
          setProgress({ stage: "complete", completed_per_mille: 1000, attempt: 1, message: null });
          setExportId(null);
          exportIdRef.current = null;
        }),
        listen<{ export_id: string; message: string; cancelled: boolean }>("recording-export-failed", ({ payload }) => {
          if (!active || payload.export_id !== exportIdRef.current) return;
          if (payload.cancelled) {
            setToast("");
            setProgress({ stage: "cancelled", completed_per_mille: 0, attempt: 0, message: null });
          } else {
            setError(payload.message);
          }
          setExportId(null);
          exportIdRef.current = null;
        }),
      ]);
      if (!artifactId) return;
      const [loaded, loadedSettings] = await Promise.all([
        invoke<RecordingArtifact | null>("get_recording_artifact", { artifactId }),
        invoke<AppSettings>("get_settings").catch(() => null),
      ]);
      if (!active || !loaded) return;
      setArtifact(loaded);
      setTrimEnd(loaded.duration_ms);
      setPlayheadMs(0);
      setCrop({ x: 0, y: 0, width: loaded.width, height: loaded.height });
      setCustomWidth(loaded.width);
      setCustomHeight(loaded.height);
      setOutputFormat(loaded.kind === "gif" ? "gif" : "mp4");
      setSizeMode(loaded.kind === "gif" ? "compress" : "preserve");
      setFilenameStem(`${recordingFileStem(loaded.path)}-edited`);
      setDestinationDirectory(recordingParentDirectory(loaded.path));
      setPreviewPlaying(false);
      void invoke<RecordingTimelinePreview>("prepare_recording_timeline_preview", {
        artifactId: loaded.id,
      }).then((preview) => {
        if (active) setTimeline(preview);
      }).catch(() => {
        if (active) setTimeline(null);
      });
      if (loadedSettings) {
        setGifFps(loadedSettings.recording.gif_fps);
        setGifMaxWidth(loadedSettings.recording.gif_max_width);
        setGifColors(loadedSettings.recording.gif_max_colors);
      }
    })().catch((error) => {
      if (active) setError(String(error));
    });
    return () => {
      active = false;
      dispose.forEach((unlisten) => unlisten());
    };
  }, [artifactId]);

  if (!artifact) {
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
  const outputDimensions = outputFormat === "gif"
    ? dimensionsAtMaximumWidth(baseOutputDimensions.width, baseOutputDimensions.height, gifMaxWidth)
    : baseOutputDimensions;
  const hasRecordedAudio = artifact.has_system_audio || artifact.has_microphone_audio;
  const sourceDirectory = recordingParentDirectory(artifact.path);

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
        await video.play();
      } else {
        video.pause();
      }
    } catch (error) {
      setPreviewPlaying(false);
      setError(`Preview could not play: ${String(error)}`);
    }
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

  const startExport = async () => {
    if (exportId) return;
    const invalidFilename = recordingFilenameError(filenameStem);
    if (invalidFilename) {
      setError(invalidFilename);
      return;
    }
    const maximumBytes = sizeMode === "maximum"
      ? Math.floor(Number(maximumSize) * FILE_SIZE_UNIT_BYTES[maximumUnit])
      : null;
    if (
      sizeMode === "maximum"
      && (maximumBytes === null || !Number.isFinite(maximumBytes) || maximumBytes < 100_000)
    ) {
      setError("Enter a maximum file size of at least 100 KB.");
      return;
    }
    const edit: EditSpec = {
      trim_start_ms: Math.round(trimStart),
      trim_end_ms: Math.round(trimEnd) >= artifact.duration_ms ? null : Math.round(trimEnd),
      crop: cropEnabled ? boundedCrop(crop, artifact.width, artifact.height) : null,
      output_width: resolution === "original" && outputFormat !== "gif" ? null : outputDimensions.width,
      output_height: resolution === "original" && outputFormat !== "gif" ? null : outputDimensions.height,
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
      gif_max_colors: outputFormat === "gif" ? gifColors : null,
    };
    setError("");
    setToast("");
    setExported(null);
    setProgress({ stage: "preparing", completed_per_mille: 0, attempt: 0, message: null });
    try {
      const id = await invoke<string>("start_recording_export", {
        request: {
          artifact_id: artifact.id,
          file_stem: filenameStem,
          destination_directory: destinationDirectory,
          edit,
          export: exportSpec,
        },
      });
      exportIdRef.current = id;
      setExportId(id);
    } catch (error) {
      setError(String(error));
      setProgress(null);
    }
  };

  return (
    <main className="recording-editor">
      <header className="recording-editor-header">
        <div><h1>{artifact.kind === "gif" ? "Edit GIF" : "Edit recording"}</h1></div>
        <div className="recording-editor-header-actions">
          <button type="button" onClick={() => {
            setError("");
            void invoke("reveal_recording_artifact", { artifactId: (exported ?? artifact).id })
              .catch((error) => setError(`Finder could not open: ${String(error)}`));
          }}>Show in Folder</button>
        </div>
      </header>
      {artifact.dropped_frames > 0 && <p className="recording-editor-warning" role="status">This source dropped {artifact.dropped_frames.toLocaleString()} frame{artifact.dropped_frames === 1 ? "" : "s"} during capture. The original timing is preserved.</p>}

      <section className="recording-editor-preview">
        <div className="recording-preview-toolbar">
          <strong>Preview</strong>
          <div className="recording-preview-toolbar-actions">
            {artifact.kind === "video" && (
              <button
                type="button"
                className="recording-preview-play"
                aria-label={previewPlaying ? "Pause preview" : "Play preview"}
                onClick={() => void togglePreviewPlayback()}
              >
                <span aria-hidden="true">{previewPlaying ? "Ⅱ" : "▶"}</span>
                {previewPlaying ? "Pause" : "Play"}
              </button>
            )}
            <div className="editor-segmented" aria-label="Preview size">
              <button type="button" className={previewMode === "fit" ? "active" : ""} onClick={() => setPreviewMode("fit")}>Fit</button>
              <button type="button" className={previewMode === "actual" ? "active" : ""} onClick={() => setPreviewMode("actual")}>100%</button>
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
                onPlay={() => setPreviewPlaying(true)}
                onPause={() => setPreviewPlaying(false)}
                onEnded={() => setPreviewPlaying(false)}
                onLoadedMetadata={(event) => {
                  event.currentTarget.currentTime = playheadMs / 1_000;
                }}
                onTimeUpdate={(event) => setPlayheadMs(event.currentTarget.currentTime * 1_000)}
                onSeeked={(event) => setPlayheadMs(event.currentTarget.currentTime * 1_000)}
              />
            ) : (
              <img src={artifact.media_url} alt="Animated GIF preview" />
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
            trimDragRef.current = null;
          }}
          onPointerCancel={() => {
            timelineScrubbingRef.current = false;
            trimDragRef.current = null;
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
            onPointerDown={(event) => {
              trimDragRef.current = "start";
              event.currentTarget.setPointerCapture(event.pointerId);
              event.preventDefault();
              event.stopPropagation();
            }}
            onPointerMove={(event) => {
              if (trimDragRef.current === "start") updateTimelinePointer(event.clientX);
            }}
            onPointerUp={() => {
              trimDragRef.current = null;
            }}
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
            onPointerDown={(event) => {
              trimDragRef.current = "end";
              event.currentTarget.setPointerCapture(event.pointerId);
              event.preventDefault();
              event.stopPropagation();
            }}
            onPointerMove={(event) => {
              if (trimDragRef.current === "end") updateTimelinePointer(event.clientX);
            }}
            onPointerUp={() => {
              trimDragRef.current = null;
            }}
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
        <section className="editor-card editor-output-card">
          <h2>Output format</h2>
          <div className="editor-segmented">
            <button type="button" className={outputFormat === "mp4" ? "active" : ""} onClick={() => setOutputFormat("mp4")}>Video (MP4)</button>
            <button
              type="button"
              className={outputFormat === "gif" ? "active" : ""}
              onClick={() => {
                setOutputFormat("gif");
                if (sizeMode === "preserve") setSizeMode("compress");
              }}
            >Animated GIF</button>
          </div>
          {outputFormat === "gif" && (
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
              <div className="editor-field"><span>Palette</span>
                <CustomSelect
                  value={String(gifColors)}
                  ariaLabel="GIF palette"
                  options={[64, 96, 128, 192, 256].map((value) => ({ value: String(value), label: `${value} colors` }))}
                  onChange={(value) => setGifColors(Number(value))}
                />
              </div>
            </div>
          )}
        </section>

        <section className="editor-card">
          <h2>Crop & size</h2>
          <label className="check-row"><input type="checkbox" checked={cropEnabled} onChange={(event) => setCropEnabled(event.target.checked)} /><span>Crop recording</span></label>
          <div className="editor-number-grid">
            <label>X<input type="number" min={0} max={Math.max(0, artifact.width - crop.width)} value={crop.x} disabled={!cropEnabled} onChange={(event) => updateCropOrigin("x", Number(event.target.value))} /></label>
            <label>Y<input type="number" min={0} max={Math.max(0, artifact.height - crop.height)} value={crop.y} disabled={!cropEnabled} onChange={(event) => updateCropOrigin("y", Number(event.target.value))} /></label>
            <label>Width<input type="number" min={2} max={Math.max(2, artifact.width - crop.x)} value={crop.width} disabled={!cropEnabled} onChange={(event) => updateCropDimension("width", Number(event.target.value))} /></label>
            <label>Height<input type="number" min={2} max={Math.max(2, artifact.height - crop.y)} value={crop.height} disabled={!cropEnabled} onChange={(event) => updateCropDimension("height", Number(event.target.value))} /></label>
          </div>
          <label className="check-row compact editor-aspect-lock"><input type="checkbox" checked={aspectLocked} onChange={(event) => setAspectLocked(event.target.checked)} /><span>Lock aspect ratio</span></label>
          <div className="editor-field editor-resolution-field"><span>Output resolution</span>
            <CustomSelect
              value={resolution}
              ariaLabel="Output resolution"
              options={[
                { value: "original", label: "Original" },
                { value: "1080", label: "1080p maximum" },
                { value: "720", label: "720p maximum" },
                { value: "custom", label: "Custom" },
              ]}
              onChange={(value) => setResolution(value as typeof resolution)}
            />
          </div>
          {resolution === "custom" && <div className="editor-number-grid dimensions"><label>Width<input type="number" min={2} value={customWidth} onChange={(event) => setCustomWidth(Number(event.target.value))} /></label><label>Height<input type="number" min={2} value={customHeight} onChange={(event) => setCustomHeight(Number(event.target.value))} /></label></div>}
          <output className="editor-output-dimensions">{outputDimensions.width} × {outputDimensions.height}</output>
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
                  description: "Keeps the recording at its best available quality.",
                }] : []),
                {
                  value: "compress",
                  label: "Compress",
                  description: "Choose a smaller file with a quality preset.",
                },
                {
                  value: "maximum",
                  label: "Maximum file size",
                  description: "Set a hard size limit for the saved file.",
                },
              ]}
              onChange={(value) => setSizeMode(value as typeof sizeMode)}
            />
          </div>
          <p className="editor-field-help">
            {sizeMode === "preserve"
              ? "Keeps the recording at its best available quality."
              : sizeMode === "compress"
                ? "Choose a smaller file with a quality preset."
                : "Set a hard size limit for the saved file."}
          </p>
          {sizeMode === "compress" && (
            <>
              <div className="editor-field"><span>Compression preset</span>
                <CustomSelect
                  value={quality}
                  ariaLabel="Compression preset"
                  options={[
                    {
                      value: "high",
                      label: "High quality",
                      description: "Larger file with the least quality loss.",
                    },
                    {
                      value: "standard",
                      label: "Balanced",
                      description: "Good quality with a smaller file.",
                    },
                    {
                      value: "small",
                      label: "Smaller file",
                      description: "Smallest file with more quality loss.",
                    },
                  ]}
                  onChange={(value) => setQuality(value as typeof quality)}
                />
              </div>
              <p className="editor-field-help">
                {quality === "high"
                  ? "Larger file with the least quality loss."
                  : quality === "standard"
                    ? "Good quality with a smaller file."
                    : "Smallest file with more quality loss."}
              </p>
            </>
          )}
          {sizeMode === "maximum" && (
            <div className="editor-field"><span>Maximum file size</span>
              <div className="editor-size-limit">
                <input
                  type="number"
                  min={maximumUnit === "kb" ? 100 : maximumUnit === "mb" ? 0.1 : 0.0001}
                  step={maximumUnit === "kb" ? 1 : maximumUnit === "mb" ? 0.1 : 0.0001}
                  value={maximumSize}
                  aria-label="Maximum file size"
                  onChange={(event) => setMaximumSize(event.target.value)}
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
        </section>

        {artifact.kind === "video" && outputFormat === "mp4" && hasRecordedAudio && <section className="editor-card editor-audio-card">
          <h2>Audio</h2>
          {artifact.has_system_audio && <label className="editor-volume"><span><input type="checkbox" checked={!muteSystem} onChange={(event) => setMuteSystem(!event.target.checked)} />System audio</span><input type="range" min={0} max={200} value={systemVolume} disabled={muteSystem} onChange={(event) => setSystemVolume(Number(event.target.value))} /><output>{systemVolume}%</output></label>}
          {artifact.has_microphone_audio && <label className="editor-volume"><span><input type="checkbox" checked={!muteMicrophone} onChange={(event) => setMuteMicrophone(!event.target.checked)} />Microphone</span><input type="range" min={0} max={200} value={microphoneVolume} disabled={muteMicrophone} onChange={(event) => setMicrophoneVolume(Number(event.target.value))} /><output>{microphoneVolume}%</output></label>}
          <label className="check-row compact"><input type="checkbox" checked={mono} onChange={(event) => setMono(event.target.checked)} /><span>Convert to mono</span></label>
        </section>}
        {artifact.kind === "video" && outputFormat === "gif" && hasRecordedAudio && <section className="editor-card editor-audio-warning" role="status">
          <h2>Audio</h2>
          <p>GIFs do not include recorded audio.</p>
        </section>}
      </div>

      <footer className={`recording-save-footer${error ? " has-error" : ""}`}>
        {progress && <div className="recording-export-progress"><span style={{ width: `${progress.completed_per_mille / 10}%` }} /></div>}
        <div className="recording-filename">
          <label htmlFor="recording-save-filename">Filename</label>
          <span className="recording-filename-input">
            <input
              id="recording-save-filename"
              value={filenameStem}
              aria-label="Saved filename"
              spellCheck={false}
              disabled={Boolean(exportId)}
              onChange={(event) => {
                setFilenameStem(event.target.value);
                setError("");
              }}
            />
            <strong>.{outputFormat}</strong>
          </span>
          <div className="recording-destination">
            <span>Save to</span>
            <output aria-label="Save location" title={destinationDirectory}>{destinationDirectory}</output>
            <button
              type="button"
              aria-label="Change save location"
              disabled={Boolean(exportId)}
              onClick={() => void chooseDestinationDirectory()}
            >Change…</button>
          </div>
        </div>
        <div className="recording-save-feedback" aria-live="polite">
          {error
            ? <p className="recording-save-error" role="alert">{error}</p>
            : toast
              ? <p className="recording-save-success" role="status">{toast}</p>
              : progress
                ? <p>{progress.message || exportStageLabel(progress.stage)}</p>
                : <p>{destinationDirectory === sourceDirectory
                  ? "Ready to save beside the source recording."
                  : "Ready to save in the selected folder."}</p>}
        </div>
        <div className="recording-save-actions">
          {exportId && <button type="button" onClick={() => void invoke("cancel_recording_export", { exportId })}>Cancel</button>}
          <button className="primary" type="button" disabled={Boolean(exportId)} onClick={() => void startExport()}>{exportId ? "Saving…" : "Save"}</button>
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

function recordingParentDirectory(path: string): string {
  const separator = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (separator < 0) return ".";
  if (separator === 0) return path.slice(0, 1);
  return path.slice(0, separator);
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
  const [hoveredWindow, setHoveredWindow] = useState<string | null>(null);
  const [selectionFeedback, setSelectionFeedback] = useState(0);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const activeSessionIdRef = useRef<string | null>(null);
  const revealingSessionIdRef = useRef<string | null>(null);
  const regionOverlayWarmedRef = useRef(false);
  const selectionFeedbackTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastRegionCursorSyncAtRef = useRef(0);
  const sessionId = session?.id ?? query("session_id");
  const mode = session?.mode ?? ((query("mode") ?? "region") as CaptureMode);

  useEffect(() => {
    let active = true;
    let dispose: (() => void) | undefined;
    void (async () => {
      dispose = await listen<ActiveSession>("capture-session-ready", ({ payload }) => {
        activeSessionIdRef.current = payload.id;
        revealingSessionIdRef.current = null;
        setVisibleSessionId(null);
        setPrimingSessionId(null);
        setSession(payload);
        setStart(null);
        setCurrent(null);
        setHoveredWindow(null);
        if (selectionFeedbackTimerRef.current) {
          clearTimeout(selectionFeedbackTimerRef.current);
          selectionFeedbackTimerRef.current = null;
        }
        setSelectionFeedback(0);
        lastRegionCursorSyncAtRef.current = 0;
      });
      const initialSession = query("session_id")
        ? await invoke<ActiveSession | null>("get_active_session", { sessionId: query("session_id") })
        : await invoke<ActiveSession | null>("get_pending_session");
      if (active && initialSession) {
        activeSessionIdRef.current = initialSession.id;
        revealingSessionIdRef.current = null;
        setVisibleSessionId(null);
        setPrimingSessionId(null);
        setSession(initialSession);
      }
    })();
    return () => {
      active = false;
      dispose?.();
    };
  }, []);

  useEffect(() => () => {
    if (selectionFeedbackTimerRef.current) clearTimeout(selectionFeedbackTimerRef.current);
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !sessionId) return;
      void invoke("cancel_capture", { sessionId });
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [sessionId]);

  useEffect(() => {
    if (mode === "region" && (!sessionId || visibleSessionId !== sessionId || primingSessionId === sessionId)) {
      return;
    }
    const cursorClass = `capture-${mode}-cursor`;
    document.documentElement.classList.add(cursorClass);
    return () => document.documentElement.classList.remove(cursorClass);
  }, [mode, primingSessionId, sessionId, visibleSessionId]);

  const rect = useMemo(
    () => (start && current ? selectionRect(start, current) : null),
    [current, start],
  );

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
    };
  }, [hoveredWindow, mode, session]);

  // ALL hooks must stay above any early return.
  const windowLayouts = useMemo(() => {
    if (mode !== "window" || !session) return [];
    const scale = Math.max(session.window_coordinate_scale || 1, 1);
    // CGWindowList is front-to-back; index 0 is topmost (highest z-index).
    return session.windows
      .filter((window) => window.width >= 48 && window.height >= 48)
      .map((window, index, list) => ({
        window,
        left: (window.x - session.display.x) / scale,
        top: (window.y - session.display.y) / scale,
        width: window.width / scale,
        height: window.height / scale,
        zIndex: list.length - index,
      }));
  }, [mode, session]);

  const revealOverlay = useCallback(async () => {
    if (!sessionId) return;
    if (revealingSessionIdRef.current === sessionId) return;
    revealingSessionIdRef.current = sessionId;
    const shouldPrimeRegionOverlay = mode === "region" && !regionOverlayWarmedRef.current;
    try {
      await invoke("show_capture_overlay", { sessionId });
    } catch {
      if (revealingSessionIdRef.current === sessionId) revealingSessionIdRef.current = null;
      return;
    }
    if (shouldPrimeRegionOverlay) {
      // Paint the stable snapshot and zero-opacity shade while native alpha is
      // 0, so revealing the window can start the shade fade without a WebKit flash.
      setPrimingSessionId(sessionId);
    }
    afterNextPaint(() => {
      if (activeSessionIdRef.current !== sessionId) return;
      void invoke("reveal_capture_overlay", { sessionId }).then(() => {
        if (shouldPrimeRegionOverlay) regionOverlayWarmedRef.current = true;
        requestAnimationFrame(() => {
          if (activeSessionIdRef.current !== sessionId) return;
          setPrimingSessionId(null);
          setVisibleSessionId(sessionId);
        });
      }).catch(() => {
        setPrimingSessionId(null);
        setVisibleSessionId(null);
        if (revealingSessionIdRef.current === sessionId) revealingSessionIdRef.current = null;
      });
    });
  }, [mode, sessionId]);

  // Safety: if snapshot onLoad never fires, still show the overlay.
  useEffect(() => {
    if (!session?.id) return;
    const timer = window.setTimeout(() => {
      void revealOverlay();
    }, 120);
    return () => window.clearTimeout(timer);
  }, [session?.id, revealOverlay]);

  if (!session || !sessionId) {
    return <main className="capture-loading">Preparing capture…</main>;
  }

  const pointFromEvent = (event: React.PointerEvent) => {
    const bounds = surfaceRef.current?.getBoundingClientRect();
    if (!bounds) return { x: 0, y: 0 };
    return { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
  };

  const commitRegion = (): boolean => {
    if (!isCapturableSelection(rect)) return false;
    void invoke("commit_region", { sessionId, rect });
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
    clearSelectionFeedback();
    reassertRegionCursor();
    event.currentTarget.setPointerCapture(event.pointerId);
    const point = pointFromEvent(event);
    setStart(point);
    setCurrent(point);
  };

  const onPointerMove = (event: React.PointerEvent) => {
    if (mode !== "region") return;
    reassertRegionCursor();
    if (!start) return;
    setCurrent(pointFromEvent(event));
  };

  const onPointerUp = () => {
    if (mode !== "region" || !start) return;
    if (!commitRegion()) showSelectionFeedback();
    setStart(null);
    setCurrent(null);
  };

  const hasSelection = Boolean(rect && rect.width > 0 && rect.height > 0);
  const dimHole = mode === "region" && hasSelection && rect
    ? { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
    : mode === "window"
      ? hoveredWindowLayout
      : null;

  return (
    <main
      key={sessionId}
      ref={surfaceRef}
      className={`capture-surface capture-${mode}${visibleSessionId === sessionId ? " capture-visible" : ""}${primingSessionId === sessionId ? " capture-priming" : ""}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
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
      <img
        className="capture-snapshot"
        src={session.snapshot_url}
        alt=""
        draggable={false}
        onLoad={() => void revealOverlay()}
        onError={() => void revealOverlay()}
      />
      <CaptureDim
        mode={mode}
        hole={dimHole}
        bounds={{ width: session.display.width, height: session.display.height }}
      />
      <div
        key={`${sessionId}-${selectionFeedback}`}
        className={`capture-hint${selectionFeedback > 0 ? " capture-hint-feedback" : ""}`}
        role="status"
        aria-live="polite"
      >
        {mode === "region"
          ? selectionFeedback > 0
            ? "Click and drag to select an area · Esc to cancel"
            : "Drag to capture · Esc to cancel"
          : "Select a window · Esc to cancel"}
      </div>
      {hasSelection && rect && (
        <div
          className="selection-box"
          style={{ left: rect.x, top: rect.y, width: rect.width, height: rect.height }}
        >
          <span>{Math.round(rect.width)} × {Math.round(rect.height)}</span>
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
              }}
              title={item.window.title || item.window.app_name || "Window"}
              onPointerEnter={() => setHoveredWindow(item.window.id)}
              onPointerLeave={() =>
                setHoveredWindow((current) => (current === item.window.id ? null : current))
              }
              onClick={() => {
                void invoke("commit_window", { sessionId, windowId: item.window.id });
              }}
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
 * shade on top. Window mode retains the parent surface entrance transition.
 */
function CaptureDim({
  mode,
  hole,
  bounds,
}: {
  mode: CaptureMode;
  hole: { x: number; y: number; width: number; height: number } | null;
  bounds: { width: number; height: number };
}) {
  // Window mode: only dim once a window is hovered so idle stays clear.
  if (mode === "window" && !hole) return null;

  if (!hole) {
    return <div className="capture-shade capture-shade-full" />;
  }

  const { x, y, width, height } = hole;
  const left = Math.max(0, Math.min(bounds.width, x));
  const top = Math.max(0, Math.min(bounds.height, y));
  const right = Math.max(left, Math.min(bounds.width, x + width));
  const bottom = Math.max(top, Math.min(bounds.height, y + height));
  const path = [
    `M0 0H${bounds.width}V${bounds.height}H0Z`,
    `M${left} ${top}H${right}V${bottom}H${left}Z`,
  ].join(" ");
  return (
    <svg
      className="capture-shade-cutout"
      viewBox={`0 0 ${bounds.width} ${bounds.height}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      <path className="capture-shade capture-shade-path" d={path} fillRule="evenodd" />
    </svg>
  );
}

export function Thumbnail() {
  const [artifacts, setArtifacts] = useState<CaptureArtifact[]>([]);
  const [clipboardState, setClipboardState] = useState<ClipboardState>({
    revision: -1,
    artifact_id: null,
  });
  const [activeViewerArtifactId, setActiveViewerArtifactId] = useState<string | null>(null);
  const stackRef = useRef<HTMLElement>(null);
  const previousArtifactCount = useRef(0);
  const applyClipboardState = useCallback((next: ClipboardState) => {
    setClipboardState((current) => reconcileClipboardState(current, next));
  }, []);

  useEffect(() => {
    let active = true;
    let dispose: (() => void)[] = [];
    void (async () => {
      const removedArtifactIds = new Set<string>();
      dispose = await Promise.all([
        listen<CaptureArtifact>("capture-completed", ({ payload }) => {
          removedArtifactIds.delete(payload.id);
          setArtifacts((current) => current.some(({ id }) => id === payload.id)
            ? current
            : [...current, payload]);
        }),
        listen<CaptureArtifact>("artifact-updated", ({ payload }) => {
          setArtifacts((current) => current.map((artifact) => artifact.id === payload.id ? payload : artifact));
        }),
        listen<string>("artifact-removed", ({ payload }) => {
          removedArtifactIds.add(payload);
          setArtifacts((current) => current.filter(({ id }) => id !== payload));
          setActiveViewerArtifactId((current) => current === payload ? null : current);
        }),
        listen<ClipboardState>("clipboard-owner-changed", ({ payload }) => {
          applyClipboardState(payload);
        }),
        listen<ViewerActivationState>("viewer-activation-changed", ({ payload }) => {
          setActiveViewerArtifactId((current) => reconcileActiveViewer(current, payload));
        }),
      ]);
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
      dispose.forEach((unlisten) => unlisten());
    };
  }, [applyClipboardState]);

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
    if (
      stackRef.current
      && shouldScrollThumbnailStackToEnd(previousArtifactCount.current, artifacts.length)
    ) {
      stackRef.current.scrollTop = stackRef.current.scrollHeight;
    }
    previousArtifactCount.current = artifacts.length;
    let cancelled = false;
    // Sync may grow the native window for new cards. It intentionally does not
    // shrink after dismissals — that recomposes WKWebView and flickers survivors.
    void invoke("sync_thumbnail_stack")
      .catch(() => undefined)
      .finally(() => {
        if (!cancelled) window.dispatchEvent(new Event("captures-thumbnail-layout-changed"));
      });
    return () => {
      cancelled = true;
    };
  }, [artifacts.length]);

  useEffect(() => {
    // Keep one native hover tracker for the lifetime of the thumbnail window.
    // Restarting it when a card is added or removed briefly clears the hover
    // presentation and releases the native pointing cursor.
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let polling = false;
    let pointingCursor = false;
    let ignoringCursorEvents = false;
    let lastCursorSyncAt = 0;

    const setPointingCursor = (pointing: boolean) => {
      document.documentElement.style.cursor = pointing ? "pointer" : "";
      const now = performance.now();
      const action = thumbnailCursorSyncAction(
        pointingCursor,
        pointing,
        now - lastCursorSyncAt,
      );
      if (!action) return;
      pointingCursor = pointing;
      lastCursorSyncAt = now;
      if (action === "reassert") {
        void invoke("reassert_thumbnail_cursor");
      } else {
        void invoke("set_thumbnail_cursor", { pointing });
      }
    };

    const setIgnoreCursorEvents = (ignore: boolean) => {
      if (ignoringCursorEvents === ignore) return;
      ignoringCursorEvents = ignore;
      // After dismiss the window may stay tall; empty space above the stack
      // must pass clicks through so it does not block the desktop.
      void invoke("set_thumbnail_ignore_cursor_events", { ignore }).catch(() => undefined);
    };

    const clearNativeClasses = () => {
      clearThumbnailNativeHover();
    };

    const clearNativeHover = () => {
      clearNativeClasses();
      setPointingCursor(false);
    };

    const stopNativeTracking = () => {
      document.documentElement.classList.remove("thumbnail-native-tracking");
      clearNativeHover();
      setIgnoreCursorEvents(false);
    };

    const applyNativeHover = (position: ThumbnailPointerPosition) => {
      document.documentElement.classList.add("thumbnail-native-tracking");
      setIgnoreCursorEvents(shouldIgnoreThumbnailCursorEvents(position));
      setPointingCursor(applyThumbnailNativeHover(position));
    };

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
      let delay = 250;
      try {
        const position = await invoke<ThumbnailPointerPosition | null>(
          "get_thumbnail_pointer_position",
        );
        if (cancelled) return;
        if (!position) {
          // A focus handoff can briefly make the native pointer query
          // unavailable. Preserve the last presentation until a real sample
          // confirms that the pointer moved away so the card cannot flash.
          delay = 40;
        } else {
          applyNativeHover(position);
          delay = 40;
        }
      } catch {
        // Treat a transient native query failure as indeterminate too. A valid
        // outside sample still clears hover immediately through applyNativeHover.
        delay = 40;
      } finally {
        polling = false;
        schedulePoll(delay);
      }
    };

    const resumePolling = () => {
      if (document.hidden) return;
      schedulePoll(0);
    };

    const pollImmediately = () => {
      if (!document.hidden) schedulePoll(0);
    };

    document.addEventListener("visibilitychange", resumePolling);
    // Clicking an inactive thumbnail briefly makes its panel key before a
    // full-size viewer takes focus. Keep the last native hover presentation
    // during that transfer; the immediate poll will reconcile it without
    // flashing the metadata and unblurred image in between.
    window.addEventListener("focus", pollImmediately);
    window.addEventListener("pageshow", resumePolling);
    window.addEventListener("captures-thumbnail-ready", pollImmediately);
    window.addEventListener("captures-thumbnail-layout-changed", pollImmediately);
    schedulePoll(0);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      document.removeEventListener("visibilitychange", resumePolling);
      window.removeEventListener("focus", pollImmediately);
      window.removeEventListener("pageshow", resumePolling);
      window.removeEventListener("captures-thumbnail-ready", pollImmediately);
      window.removeEventListener("captures-thumbnail-layout-changed", pollImmediately);
      stopNativeTracking();
    };
  }, []);

  if (artifacts.length === 0) return null;

  return (
    <main ref={stackRef} className="thumbnail-stack">
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
      {artifacts.map((artifact) => (
        <ThumbnailCard
          key={artifact.id}
          artifact={artifact}
          clipboardCurrent={clipboardState.artifact_id === artifact.id}
          viewerActive={activeViewerArtifactId === artifact.id}
          onRemoved={(artifactId) => {
            setArtifacts((current) => current.filter(({ id }) => id !== artifactId));
          }}
        />
      ))}
    </main>
  );
}

export function ThumbnailCard({
  artifact,
  clipboardCurrent,
  viewerActive,
  onRemoved,
}: {
  artifact: CaptureArtifact;
  clipboardCurrent: boolean;
  viewerActive: boolean;
  onRemoved: (artifactId: string) => void;
}) {
  const [feedback, setFeedback] = useState<"saved" | null>(null);
  const [busy, setBusy] = useState<"copied" | "saved" | null>(null);
  const [error, setError] = useState("");
  const [thumbnailReady, setThumbnailReady] = useState(false);
  const [fileDragging, setFileDragging] = useState(false);
  const [exit, setExit] = useState<"dismiss" | "delete" | null>(null);
  const [dustParticles, setDustParticles] = useState<ThumbnailDustParticle[] | null>(null);
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
  } | null>(null);
  const cardRef = useRef<HTMLElement>(null);
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
    };
  }, []);

  const showSavedFeedback = () => {
    if (isExitLocked()) return;
    setFeedback("saved");
    if (feedbackTimer.current) clearTimeout(feedbackTimer.current);
    feedbackTimer.current = setTimeout(() => {
      if (isExitLocked()) return;
      setFeedback(null);
    }, 2_000);
  };

  const runAction = async (action: string, success?: "copied" | "saved") => {
    if (isExitLocked() || isExiting) return;
    if (success && busy) return;
    setError("");
    if (success) setBusy(success);
    try {
      await invoke(action, { artifactId: artifact.id });
      if (isExitLocked()) return;
      if (success === "saved") showSavedFeedback();
    } catch (error) {
      if (isExitLocked()) return;
      setError(String(error));
    } finally {
      if (success && !isExitLocked()) setBusy(null);
    }
  };

  const openViewer = () => {
    if (isExitLocked() || isExiting) return;
    // Viewer activation rerenders both the old and new active cards. Keep the
    // actual pointer presentation in a DOM attribute React does not reconcile,
    // so that render cannot flash the bright image and metadata between polls.
    if (cardRef.current) setThumbnailNativeActiveCard(cardRef.current);
    void runAction("open_artifact_viewer");
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
        ({ result }) => {
          fileDraggingRef.current = false;
          setFileDragging(false);
          if (result === "Dropped") {
            exitWith("dismiss", "dismiss_artifact");
          }
        },
      );
    } catch (error) {
      fileDraggingRef.current = false;
      setFileDragging(false);
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
    void invoke(action, { artifactId: artifact.id })
      .then(() => onRemoved(artifact.id))
      .catch((error) => {
        // Only unlock if remove failed — otherwise the card is gone.
        exitingRef.current = false;
        setExit(null);
        setExitChrome(null);
        setDustParticles(null);
        setError(String(error));
      });
  };

  const exitWith = (kind: "dismiss" | "delete", action: string) => {
    if (isExitLocked() || isExiting) return;
    // Acquire the exit lock first so any in-flight async work becomes a no-op.
    exitingRef.current = true;
    exitAction.current = action;
    if (feedbackTimer.current) {
      clearTimeout(feedbackTimer.current);
      feedbackTimer.current = null;
    }
    // Freeze chrome *as rendered now* — never flip “Saved to Folder!” into Show in Folder.
    setExitChrome({
      feedback,
      hasPath: Boolean(artifact.path),
      clipboardCurrent,
      historySaved: artifact.history_saved,
      copyFailed: artifact.clipboard_copy_status === "failed",
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
    // WebView animation events can be skipped when Windows hides or occludes
    // this always-on-top window. Never leave a deleted card and its backend
    // artifact waiting forever for animationend.
    exitFallbackTimer.current = setTimeout(
      completeExit,
      kind === "delete" ? THUMBNAIL_DELETE_FALLBACK_MS : THUMBNAIL_DISMISS_FALLBACK_MS,
    );
  };

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
  };
  // Before a folder save: trash discards the preview (dissolve). After: trash deletes the file.
  // Close only appears once a file exists so you can hide the preview without trashing it.
  const usingDust = exit === "delete" && dustParticles !== null && dustParticles.length > 0;

  return (
    <article
      ref={cardRef}
      className={[
        "thumbnail-card",
        thumbnailReady ? "thumbnail-capture-highlight" : "",
        viewerActive && !isExiting ? "thumbnail-viewer-active" : "",
        fileDragging ? "thumbnail-file-dragging" : "",
        exit ? `thumbnail-exit-${exit}` : "",
        usingDust ? "thumbnail-exit-dust" : "",
        isExiting ? "thumbnail-exiting" : "",
      ].filter(Boolean).join(" ")}
      // HTML inert disables all descendant input/focus for the whole exit animation.
      inert={isExiting ? true : undefined}
      aria-busy={isExiting || fileDragging}
      data-exit-locked={isExiting ? "true" : undefined}
      data-file-dragging={fileDragging ? "true" : undefined}
      onAnimationEnd={finishExit}
    >
      <img
        src={artifact.full_url}
        alt="Screenshot preview"
        draggable={!isExiting}
        onDragStart={(event) => void beginFileDrag(event)}
        onLoad={markThumbnailReady}
        onError={markThumbnailReady}
      />
      {usingDust && (
        <div className="thumbnail-dust-layer" aria-hidden="true">
          {dustParticles.map((particle) => (
            <span
              key={particle.id}
              className="thumbnail-dust"
              style={{
                left: particle.left,
                top: particle.top,
                width: particle.width,
                height: particle.height,
                backgroundImage: `url(${JSON.stringify(artifact.preview_url).slice(1, -1)})`,
                backgroundSize: `${particle.surfaceWidth}px ${particle.surfaceHeight}px`,
                backgroundPosition: `${particle.bgX}px ${particle.bgY}px`,
                ["--dust-x" as string]: `${particle.dx}px`,
                ["--dust-y" as string]: `${particle.dy}px`,
                ["--dust-rotate" as string]: `${particle.rotate}deg`,
                animationDelay: `${particle.delayMs}ms`,
                animationDuration: `${particle.durationMs}ms`,
              }}
            />
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
        <div className="thumbnail-top-right">
          <IconButton
            label="Full size"
            disabled={isExiting}
            onClick={openViewer}
          >
            <ExpandIcon />
          </IconButton>
        </div>
      </div>
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
        {chrome.clipboardCurrent && (
          <div className="clipboard-confirmation" role="status">
            <CheckIcon />
            <span>Copied to clipboard</span>
          </div>
        )}
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
  return (
    <button
      type="button"
      className={`icon-button ${className}`}
      aria-label={label}
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

function ExpandIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 3H3v5M16 3h5v5M21 16v5h-5M3 16v5h5M9 9 3 3m12 6 6-6m-6 12 6 6M9 15l-6 6" /></svg>;
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
      setError(String(error));
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
              {draft.last_error && <small className="warning">{draft.last_error}</small>}
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

export function Preferences() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [recordingDevices, setRecordingDevices] = useState<AudioDevice[]>([]);
  const [saveStatus, setSaveStatus] = useState<PreferencesSaveStatus>({ kind: "idle", message: "" });
  const [recordingShortcut, setRecordingShortcut] = useState<string | null>(null);
  const settingsRef = useRef<AppSettings | null>(null);
  const pendingSettingsRef = useRef<AppSettings | null>(null);
  const saveDelayTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savedStatusTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saveInFlightRef = useRef<Promise<void> | null>(null);
  const activeRef = useRef(true);

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
    void invoke<AppSettings>("get_settings").then((loadedSettings) => {
      if (!active) return;
      settingsRef.current = loadedSettings;
      setSettings(loadedSettings);
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

  const chooseDirectory = async () => {
    const selected = await open({ directory: true, multiple: false, title: "Choose capture folder" });
    if (typeof selected === "string") update("output_directory", selected);
  };

  return (
    <main className="preferences">
      <header className="preferences-header">
        <div>
          <span className="eyebrow">Captures</span>
          <h1>Preferences</h1>
        </div>
        {saveStatus.kind !== "idle" && (
          <div className="preferences-header-actions">
            <div className={`preferences-save-status preferences-save-${saveStatus.kind}`} role="status">
              <span aria-hidden="true">{saveStatus.kind === "saved" ? "✓" : saveStatus.kind === "error" ? "!" : ""}</span>
              {saveStatus.message}
            </div>
          </div>
        )}
      </header>

      <section className="settings-section">
        <h2>Captures</h2>
        <label className="field-label" htmlFor="output-directory">Save captures to</label>
        <div className="directory-input">
          <input id="output-directory" value={settings.output_directory} onChange={(event) => update("output_directory", event.target.value)} />
          <button type="button" onClick={() => void chooseDirectory()}>Choose</button>
        </div>
        <label className="check-row capture-option">
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
      </section>

      <section className="settings-section">
        <h2>Shortcuts</h2>
        <ShortcutInput
          id="region-shortcut"
          label="Region"
          value={settings.region_shortcut}
          recording={recordingShortcut === "region-shortcut"}
          onRecordingChange={(recording) => setRecordingShortcut(recording ? "region-shortcut" : null)}
          onChange={(value) => update("region_shortcut", value)}
        />
        <ShortcutInput
          id="window-shortcut"
          label="Window"
          value={settings.window_shortcut}
          recording={recordingShortcut === "window-shortcut"}
          onRecordingChange={(recording) => setRecordingShortcut(recording ? "window-shortcut" : null)}
          onChange={(value) => update("window_shortcut", value)}
        />
        <ShortcutInput
          id="display-shortcut"
          label="Full Screen"
          value={settings.display_shortcut}
          recording={recordingShortcut === "display-shortcut"}
          onRecordingChange={(recording) => setRecordingShortcut(recording ? "display-shortcut" : null)}
          onChange={(value) => update("display_shortcut", value)}
        />
        <ShortcutInput
          id="video-shortcut"
          label="Record Screen"
          value={settings.recording.video_shortcut}
          recording={recordingShortcut === "video-shortcut"}
          onRecordingChange={(recording) => setRecordingShortcut(recording ? "video-shortcut" : null)}
          onChange={(value) => updateRecording("video_shortcut", value)}
        />
        <p className="help-text">Select a shortcut, then press the key combination you want. Press Esc to cancel recording. Changes save automatically.</p>
      </section>

      <section className="settings-section recording-settings-section">
        <h2>Video recording</h2>
        <div className="settings-inline-grid">
          <div className="settings-select-field"><span>Frames per second</span>
            <CustomSelect
              value={String(settings.recording.video_fps)}
              ariaLabel="Recording frames per second"
              options={[60, 30, 15].map((value) => ({ value: String(value), label: `${value} FPS` }))}
              onChange={(value) => updateRecording("video_fps", Number(value))}
            />
          </div>
          <div className="settings-select-field"><span>Maximum resolution</span>
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
          </div>
        </div>
        <label className="check-row capture-option"><input type="checkbox" checked={settings.recording.capture_system_audio} onChange={(event) => updateRecording("capture_system_audio", event.target.checked)} /><span>Record desktop audio<small>Captures excludes its own sounds and asks for access only when enabled.</small></span></label>
        <div className="settings-select-field field-label"><span>Default microphone</span>
          <CustomSelect
            value={settings.recording.microphone_device_id ?? "off"}
            ariaLabel="Default microphone"
            options={[
              { value: "off", label: "Off" },
              ...recordingDevices.map((device) => ({ value: device.id, label: device.name })),
            ]}
            onChange={(value) => updateRecording("microphone_device_id", value === "off" ? null : value)}
          />
        </div>
        <label className="check-row"><input type="checkbox" checked={settings.recording.mono_audio} onChange={(event) => updateRecording("mono_audio", event.target.checked)} /><span>Export recording audio in mono</span></label>
      </section>

      <section className="settings-section recording-settings-section">
        <h2>GIF export defaults</h2>
        <div className="settings-inline-grid">
          <div className="settings-select-field"><span>Frames per second</span>
            <CustomSelect value={String(settings.recording.gif_fps)} ariaLabel="GIF frames per second" options={[8, 10, 12, 15, 20, 24, 30].map((value) => ({ value: String(value), label: `${value} FPS` }))} onChange={(value) => updateRecording("gif_fps", Number(value))} />
          </div>
          <div className="settings-select-field"><span>Maximum width</span>
            <CustomSelect value={String(settings.recording.gif_max_width)} ariaLabel="GIF maximum width" options={[320, 480, 640, 800, 1200].map((value) => ({ value: String(value), label: `${value} px` }))} onChange={(value) => updateRecording("gif_max_width", Number(value))} />
          </div>
          <div className="settings-select-field"><span>Palette colors</span>
            <CustomSelect value={String(settings.recording.gif_max_colors)} ariaLabel="GIF palette colors" options={[64, 96, 128, 256].map((value) => ({ value: String(value), label: String(value) }))} onChange={(value) => updateRecording("gif_max_colors", Number(value))} />
          </div>
        </div>
      </section>

      <section className="settings-section recording-settings-section">
        <h2>Recording behavior</h2>
        <div className="settings-select-field field-label"><span>Countdown</span>
          <CustomSelect
            value={String(settings.recording.countdown_seconds)}
            ariaLabel="Recording countdown"
            options={[0, 1, 2, 3, 5, 10].map((value) => ({ value: String(value), label: value === 0 ? "Off" : `${value} seconds` }))}
            onChange={(value) => updateRecording("countdown_seconds", Number(value))}
          />
        </div>
        <label className="check-row"><input type="checkbox" checked={settings.recording.show_cursor} onChange={(event) => updateRecording("show_cursor", event.target.checked)} /><span>Show cursor in recordings</span></label>
        <label className="check-row capture-option"><input type="checkbox" checked={settings.recording.open_editor_after_recording} onChange={(event) => updateRecording("open_editor_after_recording", event.target.checked)} /><span>Open the editor after recording<small>The original is saved first, so closing the editor never loses a recording.</small></span></label>
      </section>

      <UpdatePreferences />

      <label className="check-row">
        <input type="checkbox" checked={settings.launch_at_login} onChange={(event) => update("launch_at_login", event.target.checked)} />
        <span>Launch Captures when I sign in</span>
      </label>
    </main>
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
