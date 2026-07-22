import { invoke, isTauri } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { formatFileSize } from "./lib/format";
import { reconcileClipboardState } from "./lib/clipboard";
import {
  isCapturableSelection,
  selectionRect,
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
  AppSettings,
  CaptureArtifact,
  CaptureMode,
  ClipboardState,
  HistoryEntry,
  ThumbnailPointerPosition,
  UpdateStatus,
  ViewerActivationState,
} from "./types";

const currentWindow = isTauri() ? getCurrentWindow() : null;

function query(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name);
}

function afterNextPaint(callback: () => void) {
  requestAnimationFrame(() => requestAnimationFrame(callback));
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
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    let dispose: (() => void) | undefined;

    const refresh = async () => {
      try {
        const history = await invoke<HistoryEntry[]>("get_capture_history");
        if (!active) return;
        setEntries(history);
        setError("");
      } catch (error) {
        if (active) setError(`Couldn’t load capture history: ${String(error)}`);
      } finally {
        if (active) setLoading(false);
      }
    };

    void (async () => {
      dispose = await listen("capture-history-changed", () => {
        void refresh();
      });
      await refresh();
    })();

    return () => {
      active = false;
      dispose?.();
    };
  }, []);

  return (
    <main className="capture-history">
      <header className="history-header">
        <div>
          <p className="eyebrow">LOCAL RECOVERY</p>
          <h1>Capture History</h1>
          <p>Private recovery copies for 30 days. Not your Captures folder.</p>
        </div>
        {!loading && entries.length > 0 && (
          <span className="history-count">
            {entries.length} {entries.length === 1 ? "capture" : "captures"}
          </span>
        )}
      </header>

      {error && <p className="history-error" role="alert">{error}</p>}
      {loading ? (
        <section className="history-empty" aria-live="polite">
          <span className="history-empty-icon" aria-hidden="true"><HistoryIcon /></span>
          <h2>Loading history…</h2>
        </section>
      ) : entries.length === 0 ? (
        <section className="history-empty">
          <span className="history-empty-icon" aria-hidden="true"><HistoryIcon /></span>
          <h2>No recovery copies yet</h2>
          <p>New screenshots leave a recovery copy here automatically.</p>
        </section>
      ) : (
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
      )}
    </main>
  );
}

export function HistoryCard({
  entry,
  onDeleted,
}: {
  entry: HistoryEntry;
  onDeleted: (artifactId: string) => void;
}) {
  const [busy, setBusy] = useState<"restoring" | "deleting" | null>(null);
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
    if (busy) return;
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
      <div className="history-image-wrap">
        <img src={entry.preview_url} alt="Screenshot from capture history" loading="lazy" draggable={false} />
        <span className="history-mode">{formatCaptureMode(entry.mode)}</span>
      </div>
      <div className="history-card-body">
        <time dateTime={entry.created_at}>{formatHistoryDate(entry.created_at)}</time>
        <p>{entry.width} × {entry.height} · {formatFileSize(entry.size_bytes)}</p>
        <div className="history-actions">
          <button
            type="button"
            className="history-restore"
            disabled={busy !== null}
            onClick={() => void restore()}
          >
            {restored ? <><CheckIcon />Restored</> : <><RestoreIcon />{busy === "restoring" ? "Restoring…" : "Restore"}</>}
          </button>
          <button
            type="button"
            className={confirmingDelete ? "history-delete history-delete-confirm" : "history-delete"}
            aria-label={confirmingDelete ? "Confirm permanent deletion" : "Delete from History"}
            disabled={busy !== null}
            onClick={() => void deleteFromHistory()}
          >
            <TrashIcon />
            {confirmingDelete ? "Delete forever" : "Delete"}
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
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
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
      <CaptureDim mode={mode} hole={dimHole} />
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
}: {
  mode: CaptureMode;
  hole: { x: number; y: number; width: number; height: number } | null;
}) {
  // Window mode: only dim once a window is hovered so idle stays clear.
  if (mode === "window" && !hole) return null;

  if (!hole) {
    return <div className="capture-shade capture-shade-full" />;
  }

  const { x, y, width, height } = hole;
  return (
    <div className="capture-shade-cutout" aria-hidden>
      <div className="capture-shade" style={{ top: 0, left: 0, right: 0, height: Math.max(0, y) }} />
      <div className="capture-shade" style={{ top: y, left: 0, width: Math.max(0, x), height: Math.max(0, height) }} />
      <div
        className="capture-shade"
        style={{ top: y, left: x + width, right: 0, height: Math.max(0, height) }}
      />
      <div className="capture-shade" style={{ top: y + height, left: 0, right: 0, bottom: 0 }} />
    </div>
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
  const exitAction = useRef<string | null>(null);
  /**
   * Exit lock: once true, this card is frozen for the whole dismiss/delete
   * animation. Blocks clicks, async action completions, timers, and any new
   * chrome transitions. Prefer this over ad-hoc checks when adding features.
   */
  const exitingRef = useRef(false);
  const feedbackTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

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
    const action = exitAction.current;
    exitAction.current = null;
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
        exit ? `thumbnail-exit-${exit}` : "",
        usingDust ? "thumbnail-exit-dust" : "",
        isExiting ? "thumbnail-exiting" : "",
      ].filter(Boolean).join(" ")}
      // HTML inert disables all descendant input/focus for the whole exit animation.
      inert={isExiting ? true : undefined}
      aria-busy={isExiting}
      data-exit-locked={isExiting ? "true" : undefined}
      onAnimationEnd={finishExit}
    >
      <img
        src={artifact.full_url}
        alt="Screenshot preview"
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

type PreferencesSaveStatus = {
  kind: "idle" | "saving" | "saved" | "error";
  message: string;
};

export function Preferences() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
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

  if (!settings) return <main className="preferences loading">Loading preferences…</main>;

  const update = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => {
    const current = settingsRef.current;
    if (!current || Object.is(current[key], value)) return;
    const next = { ...current, [key]: value };
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
        <p className="help-text">Select a shortcut, then press the key combination you want. Press Esc to cancel recording. Changes save automatically.</p>
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
