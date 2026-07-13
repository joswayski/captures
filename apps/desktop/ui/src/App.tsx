import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";

import { selectionRect, type SelectionPoint } from "./lib/selection";
import type {
  ActiveSession,
  AppSettings,
  CaptureArtifact,
  CaptureMode,
  WindowDescriptor,
} from "./types";

const currentWindow = isTauri() ? getCurrentWindow() : null;

function query(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name);
}

export function App() {
  const view = query("view");
  if (view === "overlay") return <CaptureOverlay />;
  if (view === "thumbnail") return <Thumbnail />;
  if (view === "preferences") return <Preferences />;
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

function CaptureOverlay() {
  const [session, setSession] = useState<ActiveSession | null>(null);
  const [start, setStart] = useState<SelectionPoint | null>(null);
  const [current, setCurrent] = useState<SelectionPoint | null>(null);
  const [hoveredWindow, setHoveredWindow] = useState<string | null>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const sessionId = session?.id ?? query("session_id");
  const mode = session?.mode ?? ((query("mode") ?? "region") as CaptureMode);

  useEffect(() => {
    let active = true;
    let dispose: (() => void) | undefined;
    void (async () => {
      dispose = await listen<ActiveSession>("capture-session-ready", ({ payload }) => {
        setSession(payload);
        setStart(null);
        setCurrent(null);
        setHoveredWindow(null);
      });
      const initialSession = query("session_id")
        ? await invoke<ActiveSession | null>("get_active_session", { sessionId: query("session_id") })
        : await invoke<ActiveSession | null>("get_pending_session");
      if (active && initialSession) setSession(initialSession);
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
      ref={surfaceRef}
      className={`capture-surface capture-${mode}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      <img
        className="capture-snapshot"
        src={session.snapshot_url}
        alt=""
        draggable={false}
        onLoad={() => void invoke("show_capture_overlay", { sessionId })}
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
      dispose = await Promise.all([
        listen<CaptureArtifact>("capture-completed", ({ payload }) => {
          setArtifacts((current) => current.some(({ id }) => id === payload.id)
            ? current
            : [...current, payload]);
        }),
        listen<CaptureArtifact>("artifact-updated", ({ payload }) => {
          setArtifacts((current) => current.map((artifact) => artifact.id === payload.id ? payload : artifact));
        }),
        listen<string>("artifact-removed", ({ payload }) => {
          setArtifacts((current) => current.filter(({ id }) => id !== payload));
        }),
      ]);
      const initialArtifacts = await invoke<CaptureArtifact[]>("get_artifacts");
      if (active) setArtifacts(initialArtifacts);
    })();
    return () => {
      active = false;
      dispose.forEach((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    if (stackRef.current) stackRef.current.scrollTop = stackRef.current.scrollHeight;
  }, [artifacts.length]);

  if (artifacts.length === 0) return null;

  return (
    <main ref={stackRef} className="thumbnail-stack">
      {artifacts.map((artifact) => <ThumbnailCard key={artifact.id} artifact={artifact} />)}
    </main>
  );
}

function ThumbnailCard({ artifact }: { artifact: CaptureArtifact }) {
  const [feedback, setFeedback] = useState<"copied" | "saved" | null>(null);
  const [busy, setBusy] = useState<"copied" | "saved" | null>(null);
  const [error, setError] = useState("");
  const [exit, setExit] = useState<"dismiss" | "delete" | null>(null);
  const feedbackTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => {
    if (feedbackTimer.current) clearTimeout(feedbackTimer.current);
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
    setExit(kind);
    setTimeout(() => {
      void invoke(action, { artifactId: artifact.id }).catch((error) => {
        setExit(null);
        setError(String(error));
      });
    }, 320);
  };

  return (
    <article className={`thumbnail-card ${exit ? `thumbnail-exit-${exit}` : ""}`}>
      <img src={artifact.preview_url} alt="Screenshot preview" />
      <div className="thumbnail-top-actions">
        <IconButton className="delete" label="Delete" onClick={() => exitWith("delete", "trash_artifact")}>
          <TrashIcon />
        </IconButton>
        <div className="thumbnail-top-right">
          {artifact.path && (
            <IconButton label="Show in Folder" onClick={() => void runAction("reveal_artifact")}>
              <FolderIcon />
            </IconButton>
          )}
          <button type="button" className="dismiss-button" onClick={() => exitWith("dismiss", "dismiss_artifact")}>Dismiss</button>
        </div>
      </div>
      <div className="thumbnail-main-actions">
        <button type="button" disabled={busy !== null} onClick={() => void runAction("copy_artifact", "copied")}>
          {feedback === "copied" ? <><CheckIcon />Copied!</> : <><CopyIcon />Copy</>}
        </button>
        <button type="button" disabled={busy !== null} onClick={() => void runAction("save_artifact", "saved")}>
          {feedback === "saved" ? <><CheckIcon />Saved!</> : <><SaveIcon />Save</>}
        </button>
      </div>
      <div className="thumbnail-meta">
        <span>{artifact.width} × {artifact.height}</span>
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
