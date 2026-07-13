import { invoke, isTauri } from "@tauri-apps/api/core";
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
  const sessionId = query("session_id");
  const mode = (query("mode") ?? "region") as CaptureMode;
  const [session, setSession] = useState<ActiveSession | null>(null);
  const [start, setStart] = useState<SelectionPoint | null>(null);
  const [current, setCurrent] = useState<SelectionPoint | null>(null);
  const [hoveredWindow, setHoveredWindow] = useState<string | null>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!sessionId) return;
    void invoke<ActiveSession | null>("get_active_session", { sessionId }).then(setSession);
  }, [sessionId]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !sessionId) return;
      void invoke("cancel_capture", { sessionId }).finally(() => void currentWindow?.close());
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
    void invoke("commit_region", { sessionId, rect }).finally(() => void currentWindow?.close());
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
      style={{ backgroundImage: `url(${session.snapshot_url})` }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
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
          scale={session.display.scale_factor}
          hoveredWindow={hoveredWindow}
          onHover={setHoveredWindow}
          onSelect={(window) => {
            void invoke("commit_window", { sessionId, windowId: window.id }).finally(() => void currentWindow?.close());
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
      {windows.map((window) => (
        <button
          type="button"
          key={window.id}
          className={`window-target ${hoveredWindow === window.id ? "window-target-hovered" : ""}`}
          style={{
            left: (window.x - display.x) / scale,
            top: (window.y - display.y) / scale,
            width: window.width / scale,
            height: window.height / scale,
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
  const artifactId = query("artifact_id");
  const [artifact, setArtifact] = useState<CaptureArtifact | null>(null);
  const [paused, setPaused] = useState(false);
  const [message, setMessage] = useState("");
  const timerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const close = () => {
    void invoke("close_thumbnail").finally(() => void currentWindow?.close());
  };

  useEffect(() => {
    void invoke<CaptureArtifact | null>("get_last_artifact").then(setArtifact);
  }, []);

  useEffect(() => {
    if (!artifact || paused) return;
    timerRef.current = setTimeout(close, 10_000);
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, [artifact, paused]);

  const runAction = async (action: string, success: string) => {
    if (!artifactId) return;
    try {
      await invoke(action, { artifactId });
      setMessage(success);
    } catch (error) {
      setMessage(String(error));
    }
  };

  if (!artifact) return null;

  return (
    <main
      className="thumbnail-card"
      onPointerEnter={() => setPaused(true)}
      onPointerLeave={() => setPaused(false)}
    >
      <img src={artifact.preview_url} alt="Latest capture" />
      <div className="thumbnail-meta">
        <span>{artifact.width} × {artifact.height}</span>
        {!artifact.clipboard_copied && <span className="warning">Clipboard unavailable</span>}
      </div>
      <div className="thumbnail-actions">
        <button type="button" onClick={() => void runAction("copy_artifact", "Copied")}>Copy</button>
        <button type="button" onClick={() => void runAction("reveal_artifact", "Revealed")}>Reveal</button>
        <button type="button" onClick={() => void runAction("trash_artifact", "Moved to Trash")}>Trash</button>
        <button type="button" className="quiet" onClick={close}>Dismiss</button>
      </div>
      {message && <p className="thumbnail-message">{message}</p>}
    </main>
  );
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
