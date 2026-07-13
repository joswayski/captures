import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";

import { formatFileSize } from "./lib/format";
import { selectionRect, type SelectionPoint } from "./lib/selection";
import type {
  ActiveSession,
  AppSettings,
  CaptureArtifact,
  CaptureMode,
  ThumbnailPointerPosition,
  WindowDescriptor,
} from "./types";

const currentWindow = isTauri() ? getCurrentWindow() : null;

function query(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name);
}

function afterNextPaint(callback: () => void) {
  requestAnimationFrame(() => requestAnimationFrame(callback));
}

export function App() {
  const view = query("view");
  if (view === "overlay") return <CaptureOverlay />;
  if (view === "thumbnail") return <Thumbnail />;
  if (view === "viewer") return <ArtifactViewer />;
  if (view === "preferences") return <Preferences />;
  if (view === "startup") return <StartupNotice />;
  return <IdleView />;
}

function IdleView() {
  return (
    <main className="idle-view">
      <div className="brand-mark">CES</div>
      <h1>CES is running</h1>
      <p>Use the capture shortcut or the tray icon to take a screenshot.</p>
    </main>
  );
}

function StartupNotice() {
  return (
    <main className="startup-notice">
      <div className="startup-camera" aria-hidden="true">
        <svg viewBox="0 0 24 24">
          <path d="M4 8a2 2 0 0 1 2-2h3l1.4-2h3.2L15 6h3a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2Z" />
          <circle cx="12" cy="12.5" r="3.5" />
        </svg>
      </div>
      <div>
        <strong>CES is running</strong>
        <p>Use the menu-bar camera or Ctrl+Shift+4 to capture.</p>
      </div>
    </main>
  );
}

function ArtifactViewer() {
  const artifactId = query("artifact_id");
  const [artifact, setArtifact] = useState<CaptureArtifact | null>(null);
  const [fit, setFit] = useState(true);
  const [revision, setRevision] = useState(0);
  const displayedArtifactId = useRef<string | null>(artifactId);

  useEffect(() => {
    let active = true;
    let dispose: (() => void)[] = [];
    void (async () => {
      dispose = await Promise.all([
        listen<CaptureArtifact>("viewer-artifact-changed", ({ payload }) => {
          displayedArtifactId.current = payload.id;
          setArtifact(payload);
          setRevision((current) => current + 1);
          setFit(true);
        }),
        listen<string>("artifact-removed", ({ payload }) => {
          if (displayedArtifactId.current === payload) void currentWindow?.close();
        }),
      ]);
      if (!artifactId) return;
      const initialArtifact = await invoke<CaptureArtifact | null>("get_artifact", { artifactId });
      if (active) {
        displayedArtifactId.current = initialArtifact?.id ?? null;
        setArtifact(initialArtifact);
      }
    })();
    return () => {
      active = false;
      dispose.forEach((unlisten) => unlisten());
    };
  }, [artifactId]);

  const revealViewer = (loadedArtifactId: string) => {
    afterNextPaint(() => {
      void invoke("show_artifact_viewer", { artifactId: loadedArtifactId });
    });
  };

  if (!artifact) return <main className="viewer-loading">Capture unavailable</main>;

  return (
    <main className="artifact-viewer">
      <header className="viewer-toolbar">
        <div>
          <strong>CES Preview</strong>
          <span>{artifact.width} × {artifact.height}</span>
        </div>
        <button type="button" onClick={() => setFit((current) => !current)}>
          {fit ? "Actual size" : "Fit to window"}
        </button>
      </header>
      <div className="viewer-canvas" onDoubleClick={() => setFit((current) => !current)}>
        <img
          key={`${artifact.id}-${revision}`}
          className={fit ? "viewer-image viewer-image-fit" : "viewer-image viewer-image-actual"}
          src={artifact.full_url}
          alt="Full-size screenshot"
          draggable={false}
          onLoad={() => revealViewer(artifact.id)}
          onError={() => revealViewer(artifact.id)}
        />
      </div>
    </main>
  );
}

function CaptureOverlay() {
  const [session, setSession] = useState<ActiveSession | null>(null);
  const [visibleSessionId, setVisibleSessionId] = useState<string | null>(null);
  const [start, setStart] = useState<SelectionPoint | null>(null);
  const [current, setCurrent] = useState<SelectionPoint | null>(null);
  const [hoveredWindow, setHoveredWindow] = useState<string | null>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const activeSessionIdRef = useRef<string | null>(null);
  const revealingSessionIdRef = useRef<string | null>(null);
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
        setSession(payload);
        setStart(null);
        setCurrent(null);
        setHoveredWindow(null);
      });
      const initialSession = query("session_id")
        ? await invoke<ActiveSession | null>("get_active_session", { sessionId: query("session_id") })
        : await invoke<ActiveSession | null>("get_pending_session");
      if (active && initialSession) {
        activeSessionIdRef.current = initialSession.id;
        revealingSessionIdRef.current = null;
        setVisibleSessionId(null);
        setSession(initialSession);
      }
    })();
    return () => {
      active = false;
      dispose?.();
    };
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !sessionId) return;
      void invoke("cancel_capture", { sessionId });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [sessionId]);

  const rect = useMemo(
    () => (start && current ? selectionRect(start, current) : null),
    [current, start],
  );

  if (!session || !sessionId) {
    return <main className="capture-loading">Preparing capture…</main>;
  }

  const pointFromEvent = (event: React.PointerEvent) => {
    const bounds = surfaceRef.current?.getBoundingClientRect();
    if (!bounds) return { x: 0, y: 0 };
    return { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
  };

  const commitRegion = () => {
    if (!rect || rect.width < 2 || rect.height < 2) return;
    void invoke("commit_region", { sessionId, rect });
  };

  const revealOverlay = async () => {
    if (revealingSessionIdRef.current === sessionId) return;
    revealingSessionIdRef.current = sessionId;
    try {
      await invoke("show_capture_overlay", { sessionId });
    } catch {
      if (revealingSessionIdRef.current === sessionId) revealingSessionIdRef.current = null;
      return;
    }
    afterNextPaint(() => {
      if (activeSessionIdRef.current !== sessionId) return;
      void invoke("reveal_capture_overlay", { sessionId }).then(() => {
        requestAnimationFrame(() => {
          if (activeSessionIdRef.current === sessionId) setVisibleSessionId(sessionId);
        });
      }).catch(() => {
        if (revealingSessionIdRef.current === sessionId) revealingSessionIdRef.current = null;
      });
    });
  };

  const onPointerDown = (event: React.PointerEvent) => {
    if (mode !== "region") return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const point = pointFromEvent(event);
    setStart(point);
    setCurrent(point);
  };

  const onPointerMove = (event: React.PointerEvent) => {
    if (mode !== "region" || !start) return;
    setCurrent(pointFromEvent(event));
  };

  const onPointerUp = () => {
    if (mode !== "region" || !start) return;
    commitRegion();
    setStart(null);
    setCurrent(null);
  };

  return (
    <main
      key={sessionId}
      ref={surfaceRef}
      className={`capture-surface capture-${mode}${visibleSessionId === sessionId ? " capture-visible" : ""}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      <img
        className="capture-snapshot"
        src={session.snapshot_url}
        alt=""
        draggable={false}
        onLoad={() => void revealOverlay()}
      />
      <div className="capture-shade" />
      <div className="capture-hint">
        {mode === "region" ? "Drag to capture · Esc to cancel" : "Select a window · Esc to cancel"}
      </div>
      {rect && rect.width > 0 && rect.height > 0 && (
        <div
          className="selection-box"
          style={{ left: rect.x, top: rect.y, width: rect.width, height: rect.height }}
        >
          <span>{Math.round(rect.width)} × {Math.round(rect.height)}</span>
        </div>
      )}
      {mode === "window" && (
        <WindowTargets
          display={session.display}
          windows={session.windows}
          scale={session.window_coordinate_scale}
          hoveredWindow={hoveredWindow}
          onHover={setHoveredWindow}
          onSelect={(window) => {
            void invoke("commit_window", { sessionId, windowId: window.id });
          }}
        />
      )}
    </main>
  );
}

function WindowTargets({
  display,
  windows,
  scale,
  hoveredWindow,
  onHover,
  onSelect,
}: {
  display: ActiveSession["display"];
  windows: WindowDescriptor[];
  scale: number;
  hoveredWindow: string | null;
  onHover: (id: string | null) => void;
  onSelect: (window: WindowDescriptor) => void;
}) {
  return (
    <div className="window-targets">
      {windows.map((window, index) => (
        <button
          type="button"
          key={window.id}
          className={`window-target ${hoveredWindow === window.id ? "window-target-hovered" : ""}`}
          style={{
            left: (window.x - display.x) / scale,
            top: (window.y - display.y) / scale,
            width: window.width / scale,
            height: window.height / scale,
            zIndex: windows.length - index,
          }}
          title={window.title}
          onPointerEnter={() => onHover(window.id)}
          onPointerLeave={() => onHover(null)}
          onClick={() => onSelect(window)}
        >
          <span>{window.title || window.app_name || "Window"}</span>
        </button>
      ))}
    </div>
  );
}

function Thumbnail() {
  const [artifacts, setArtifacts] = useState<CaptureArtifact[]>([]);
  const stackRef = useRef<HTMLElement>(null);

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
  }, []);

  useEffect(() => {
    if (stackRef.current) stackRef.current.scrollTop = stackRef.current.scrollHeight;
    let cancelled = false;
    afterNextPaint(() => {
      if (!cancelled) void invoke("sync_thumbnail_stack");
    });
    return () => {
      cancelled = true;
    };
  }, [artifacts.length]);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let polling = false;
    let pointingCursor = false;

    const setPointingCursor = (pointing: boolean) => {
      document.documentElement.style.cursor = pointing ? "pointer" : "";
      if (pointingCursor === pointing && !pointing) return;
      pointingCursor = pointing;
      void invoke("set_thumbnail_cursor", { pointing });
    };

    const clearNativeClasses = () => {
      document.querySelectorAll(".thumbnail-card-native-active, .native-pointer-hover")
        .forEach((element) => {
          element.classList.remove("thumbnail-card-native-active", "native-pointer-hover");
        });
    };

    const clearNativeHover = () => {
      clearNativeClasses();
      setPointingCursor(false);
    };

    const stopNativeTracking = () => {
      document.documentElement.classList.remove("thumbnail-native-tracking");
      clearNativeHover();
    };

    if (artifacts.length === 0) {
      stopNativeTracking();
      return stopNativeTracking;
    }

    const applyNativeHover = (position: ThumbnailPointerPosition) => {
      document.documentElement.classList.add("thumbnail-native-tracking");
      clearNativeClasses();
      if (!position.inside) {
        setPointingCursor(false);
        return;
      }
      const target = document.elementFromPoint(position.x, position.y);
      target?.closest(".thumbnail-card")?.classList.add("thumbnail-card-native-active");
      const button = target?.closest("button");
      if (button) {
        button.classList.add("native-pointer-hover");
      }
      setPointingCursor(Boolean(button));
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
          stopNativeTracking();
        } else {
          applyNativeHover(position);
          delay = 40;
        }
      } catch {
        if (!cancelled) stopNativeTracking();
      } finally {
        polling = false;
        schedulePoll(delay);
      }
    };

    const resumePolling = () => {
      if (document.hidden) return;
      stopNativeTracking();
      schedulePoll(0);
    };

    document.addEventListener("visibilitychange", resumePolling);
    window.addEventListener("focus", resumePolling);
    window.addEventListener("pageshow", resumePolling);
    window.addEventListener("ces-thumbnail-ready", resumePolling);
    schedulePoll(0);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      document.removeEventListener("visibilitychange", resumePolling);
      window.removeEventListener("focus", resumePolling);
      window.removeEventListener("pageshow", resumePolling);
      window.removeEventListener("ces-thumbnail-ready", resumePolling);
      stopNativeTracking();
    };
  }, [artifacts.length]);

  if (artifacts.length === 0) return null;

  return (
    <main ref={stackRef} className="thumbnail-stack">
      {artifacts.map((artifact) => (
        <ThumbnailCard
          key={artifact.id}
          artifact={artifact}
          onRemoved={(artifactId) => {
            setArtifacts((current) => current.filter(({ id }) => id !== artifactId));
          }}
        />
      ))}
    </main>
  );
}

function ThumbnailCard({
  artifact,
  onRemoved,
}: {
  artifact: CaptureArtifact;
  onRemoved: (artifactId: string) => void;
}) {
  const [feedback, setFeedback] = useState<"copied" | "saved" | null>(null);
  const [busy, setBusy] = useState<"copied" | "saved" | null>(null);
  const [error, setError] = useState("");
  const [exit, setExit] = useState<"dismiss" | "delete" | null>(null);
  const exitAction = useRef<string | null>(null);
  const feedbackTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const markThumbnailReady = () => {
    void invoke("thumbnail_ready", { artifactId: artifact.id })
      .catch(() => undefined)
      .finally(() => {
        window.dispatchEvent(new Event("ces-thumbnail-ready"));
      });
  };

  useEffect(() => {
    return () => {
      if (feedbackTimer.current) clearTimeout(feedbackTimer.current);
    };
  }, []);

  const showFeedback = (value: "copied" | "saved") => {
    setFeedback(value);
    if (feedbackTimer.current) clearTimeout(feedbackTimer.current);
    feedbackTimer.current = setTimeout(() => setFeedback(null), 2_000);
  };

  const runAction = async (action: string, success?: "copied" | "saved") => {
    if (success && busy) return;
    setError("");
    if (success) setBusy(success);
    try {
      await invoke(action, { artifactId: artifact.id });
      if (success) showFeedback(success);
    } catch (error) {
      setError(String(error));
    } finally {
      if (success) setBusy(null);
    }
  };

  const exitWith = (kind: "dismiss" | "delete", action: string) => {
    if (exit) return;
    exitAction.current = action;
    setExit(kind);
  };

  const finishExit = (event: React.AnimationEvent<HTMLElement>) => {
    if (!exit || event.animationName !== `thumbnail-${exit}` || !exitAction.current) return;
    const action = exitAction.current;
    exitAction.current = null;
    void invoke(action, { artifactId: artifact.id })
      .then(() => onRemoved(artifact.id))
      .catch((error) => {
        setExit(null);
        setError(String(error));
      });
  };

  return (
    <article
      className={`thumbnail-card ${exit ? `thumbnail-exit-${exit}` : ""}`}
      onAnimationEnd={finishExit}
    >
      <img
        src={artifact.preview_url}
        alt="Screenshot preview"
        onLoad={markThumbnailReady}
        onError={markThumbnailReady}
      />
      <div className="thumbnail-top-actions">
        <IconButton className="delete" label="Delete" onClick={() => exitWith("delete", "trash_artifact")}>
          <TrashIcon />
        </IconButton>
        <div className="thumbnail-top-right">
          <IconButton label="Open Preview" onClick={() => void runAction("open_artifact_viewer")}>
            <ExpandIcon />
          </IconButton>
          <button type="button" className="dismiss-button" onClick={() => exitWith("dismiss", "dismiss_artifact")}>Dismiss</button>
        </div>
      </div>
      <div className="thumbnail-main-actions">
        <button type="button" disabled={busy !== null} onClick={() => void runAction("copy_artifact", "copied")}>
          {feedback === "copied" ? <><CheckIcon />Copied!</> : <><CopyIcon />Copy</>}
        </button>
        <button
          type="button"
          disabled={busy !== null}
          onClick={() => void runAction(artifact.path ? "reveal_artifact" : "save_artifact", artifact.path ? undefined : "saved")}
        >
          {feedback === "saved"
            ? <><CheckIcon />Saved!</>
            : artifact.path
              ? <><FolderIcon />Show in Folder</>
              : <><SaveIcon />Save</>}
        </button>
      </div>
      <div className="thumbnail-meta">
        <span>{artifact.width} × {artifact.height} · {formatFileSize(artifact.size_bytes)}</span>
        {!artifact.clipboard_copied && <span className="warning">Clipboard unavailable</span>}
      </div>
      {error && <p className="thumbnail-message">{error}</p>}
    </article>
  );
}

function IconButton({
  children,
  className = "",
  label,
  onClick,
}: {
  children: React.ReactNode;
  className?: string;
  label: string;
  onClick: () => void;
}) {
  return (
    <button type="button" className={`icon-button ${className}`} aria-label={label} data-tooltip={label} onClick={onClick}>
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

function SaveIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 4h12l2 2v14H5Z" /><path d="M8 4v6h8V4M8 20v-6h8v6" /></svg>;
}

function CheckIcon() {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m5 12 4 4L19 6" /></svg>;
}

function Preferences() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [message, setMessage] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void invoke<AppSettings>("get_settings").then(setSettings);
  }, []);

  if (!settings) return <main className="preferences loading">Loading preferences…</main>;

  const update = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) => {
    setSettings((current) => current ? { ...current, [key]: value } : current);
  };

  const chooseDirectory = async () => {
    const selected = await open({ directory: true, multiple: false, title: "Choose capture folder" });
    if (typeof selected === "string") update("output_directory", selected);
  };

  const save = async () => {
    setSaving(true);
    setMessage("");
    try {
      await invoke("update_settings", { settings });
      setMessage("Saved");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <main className="preferences">
      <header className="preferences-header">
        <div>
          <span className="eyebrow">CES</span>
          <h1>Preferences</h1>
        </div>
        <button type="button" className="close-button" onClick={() => void currentWindow?.close()}>×</button>
      </header>

      <section className="settings-section">
        <h2>Captures</h2>
        <label className="field-label" htmlFor="output-directory">Save captures to</label>
        <div className="directory-input">
          <input id="output-directory" value={settings.output_directory} onChange={(event) => update("output_directory", event.target.value)} />
          <button type="button" onClick={() => void chooseDirectory()}>Choose</button>
        </div>
      </section>

      <section className="settings-section">
        <h2>Shortcuts</h2>
        <ShortcutInput label="Region" value={settings.region_shortcut} onChange={(value) => update("region_shortcut", value)} />
        <ShortcutInput label="Window" value={settings.window_shortcut} onChange={(value) => update("window_shortcut", value)} />
        <ShortcutInput label="Display" value={settings.display_shortcut} onChange={(value) => update("display_shortcut", value)} />
        <p className="help-text">Use the format Ctrl+Shift+4. Changes apply immediately after saving.</p>
      </section>

      <label className="check-row">
        <input type="checkbox" checked={settings.launch_at_login} onChange={(event) => update("launch_at_login", event.target.checked)} />
        <span>Launch CES when I sign in</span>
      </label>

      <footer className="preferences-footer">
        <span className="save-message">{message}</span>
        <div>
          <button type="button" className="quiet" onClick={() => void currentWindow?.close()}>Cancel</button>
          <button type="button" className="primary" disabled={saving} onClick={() => void save()}>{saving ? "Saving…" : "Save"}</button>
        </div>
      </footer>
    </main>
  );
}

function ShortcutInput({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  return (
    <label className="shortcut-row">
      <span>{label}</span>
      <input value={value} onChange={(event) => onChange(event.target.value)} spellCheck={false} />
    </label>
  );
}
