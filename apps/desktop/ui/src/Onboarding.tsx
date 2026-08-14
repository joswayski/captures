import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState } from "react";

import type { OnboardingState } from "./types";

type BusyAction = "permission" | "microphone" | "restart" | "complete" | null;

const PERMISSION_POLL_MS = 1_500;
const SETTINGS_AWAY_MS = 2_500;

function CaptureSetupIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M9 4H7a3 3 0 0 0-3 3v2M15 4h2a3 3 0 0 1 3 3v2M20 15v2a3 3 0 0 1-3 3h-2M9 20H7a3 3 0 0 1-3-3v-2" />
      <path className="onboarding-icon-fill" d="M12 8.5c.4 1.8 1.7 3.1 3.5 3.5-1.8.4-3.1 1.7-3.5 3.5-.4-1.8-1.7-3.1-3.5-3.5 1.8-.4 3.1-1.7 3.5-3.5Z" />
    </svg>
  );
}

function ScreenAccessIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="3" y="4" width="18" height="13" rx="2.5" />
      <path d="M8 21h8M12 17v4" />
      <path className="onboarding-icon-fill" d="M12 8c.35 1.55 1.45 2.65 3 3-1.55.35-2.65 1.45-3 3-.35-1.55-1.45-2.65-3-3 1.55-.35 2.65-1.45 3-3Z" />
    </svg>
  );
}

function MicrophoneAccessIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M6 11a6 6 0 0 0 12 0M12 17v4M9 21h6" />
    </svg>
  );
}

function ArrowIcon() {
  return <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M4 10h11M11 6l4 4-4 4" /></svg>;
}

function setupTitle(platform: string | undefined) {
  if (platform === "macos") return "Allow screen access";
  return "You’re ready to capture";
}

function setupDescription(platform: string | undefined) {
  if (platform === "macos") {
    return "Screen Recording is required. Microphone is optional — allow it now if you don’t want a prompt later. Pick audio sources when you record.";
  }
  return "Screen capture is available on this computer. Pick desktop audio and a microphone when you start a recording.";
}

function screenAccessDescription(platform: string, required: boolean, restartRequired: boolean, stillOff: boolean) {
  if (platform === "macos" && stillOff) {
    return "The switch for this copy of Captures is still off. A local build is a different row from a downloaded app. Turn it on, then restart.";
  }
  if (platform === "macos" && restartRequired) {
    return "Turn the switch on next to this copy of Captures, then restart. A local build is a different row from a downloaded app. macOS does not apply the permission until Captures relaunches.";
  }
  if (platform === "macos") {
    return "Allow Screen Recording so Captures can read the pixels you choose to capture. macOS keeps everything else hidden.";
  }
  if (platform === "windows") {
    return "Windows provides screen capture access without a separate permission prompt. Secure and protected windows remain private.";
  }
  if (platform === "linux") {
    return "Your desktop may show its own screen-sharing picker when a capture starts. There is nothing to approve ahead of time.";
  }
  return required
    ? "Allow your operating system to share the part of the screen you choose to capture."
    : "Screen capture is available without an additional setup step.";
}

function microphoneStatus(setup: OnboardingState, asked: boolean) {
  if (setup.microphone_granted) return "Allowed";
  if (asked || !setup.microphone_can_request) return "Needs approval";
  return "Not requested";
}

function microphoneDescription(granted: boolean) {
  if (granted) {
    return "macOS will not ask again. Turn the microphone on when you start a recording.";
  }
  return "A separate microphone permission. Allow it now so a recording does not pause to ask, or wait until you pick a mic.";
}

export function Onboarding() {
  const [setup, setSetup] = useState<OnboardingState | null>(null);
  const [busy, setBusy] = useState<BusyAction>(null);
  const [error, setError] = useState("");
  const [microphoneAsked, setMicrophoneAsked] = useState(false);
  const [leftForSettings, setLeftForSettings] = useState(false);
  const blurredAtRef = useRef<number | null>(null);
  const restartingRef = useRef(false);

  const refresh = useCallback(async () => {
    try {
      setSetup(await invoke<OnboardingState>("get_onboarding_state"));
    } catch (refreshError) {
      setError(`Couldn’t check screen access: ${String(refreshError)}`);
    }
  }, []);

  useEffect(() => {
    let active = true;
    const load = async () => {
      try {
        const next = await invoke<OnboardingState>("get_onboarding_state");
        if (active) setSetup(next);
      } catch (loadError) {
        if (active) setError(`Couldn’t load setup: ${String(loadError)}`);
      }
    };
    const handleFocus = () => void load();
    const handleVisibility = () => {
      if (document.visibilityState === "visible") void load();
    };
    void load();
    window.addEventListener("focus", handleFocus);
    document.addEventListener("visibilitychange", handleVisibility);
    let unlistenFocus: (() => void) | undefined;
    if (isTauri()) {
      void getCurrentWindow()
        .onFocusChanged(({ payload: focused }) => {
          if (focused) void load();
        })
        .then((stop) => {
          if (!active) stop();
          else unlistenFocus = stop;
        });
    }
    return () => {
      active = false;
      unlistenFocus?.();
      window.removeEventListener("focus", handleFocus);
      document.removeEventListener("visibilitychange", handleVisibility);
    };
  }, []);

  useEffect(() => {
    const waitingForScreen = Boolean(
      setup?.screen_recording_required
        && !setup.screen_recording_granted
        && (!setup.screen_recording_can_request || setup.screen_recording_requested_this_launch),
    );
    const waitingForMicrophone = Boolean(
      microphoneAsked && setup && !setup.microphone_granted,
    );
    if (!waitingForScreen && !waitingForMicrophone) return undefined;
    const timer = window.setInterval(() => void refresh(), PERMISSION_POLL_MS);
    return () => window.clearInterval(timer);
  }, [microphoneAsked, refresh, setup]);

  const restart = useCallback(() => {
    if (restartingRef.current) return;
    restartingRef.current = true;
    setBusy("restart");
    setError("");
    void invoke("restart_captures_for_permissions").catch((restartError) => {
      restartingRef.current = false;
      setBusy(null);
      setError(`Couldn’t restart Captures: ${String(restartError)}`);
    });
  }, []);

  const handleReturnedFromSettings = useCallback(async () => {
    const blurredAt = blurredAtRef.current;
    blurredAtRef.current = null;
    if (!leftForSettings || restartingRef.current) return;
    try {
      const next = await invoke<OnboardingState>("get_onboarding_state");
      setSetup(next);
      if (!next.screen_recording_required || next.screen_recording_granted) return;
      if (blurredAt == null || Date.now() - blurredAt < SETTINGS_AWAY_MS) return;
      restart();
    } catch (refreshError) {
      setError(`Couldn’t check screen access: ${String(refreshError)}`);
    }
  }, [leftForSettings, restart]);

  useEffect(() => {
    if (!leftForSettings) return undefined;
    blurredAtRef.current = Date.now();
    const onBlur = () => {
      blurredAtRef.current = Date.now();
    };
    const onFocus = () => {
      void handleReturnedFromSettings();
    };
    const onVisibility = () => {
      if (document.visibilityState === "hidden") onBlur();
      else onFocus();
    };
    window.addEventListener("blur", onBlur);
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [handleReturnedFromSettings, leftForSettings]);

  const requestPermission = async () => {
    setBusy("permission");
    setError("");
    try {
      const next = await invoke<OnboardingState>("request_onboarding_screen_permission");
      setSetup(next);
      if (!next.screen_recording_granted && !next.screen_recording_can_request) {
        setLeftForSettings(true);
      }
    } catch (permissionError) {
      setError(`Couldn’t open screen access: ${String(permissionError)}`);
    } finally {
      setBusy(null);
    }
  };

  const requestMicrophone = async () => {
    setBusy("microphone");
    setError("");
    setMicrophoneAsked(true);
    try {
      setSetup(await invoke<OnboardingState>("request_onboarding_microphone_permission"));
    } catch (microphoneError) {
      setError(`Couldn’t open microphone access: ${String(microphoneError)}`);
    } finally {
      setBusy(null);
    }
  };

  const complete = async () => {
    setBusy("complete");
    setError("");
    try {
      await invoke("complete_onboarding");
      if (isTauri()) await getCurrentWindow().close();
    } catch (completeError) {
      setBusy(null);
      setError(`Couldn’t finish setup: ${String(completeError)}`);
    }
  };

  const screenReady = Boolean(
    setup && (!setup.screen_recording_required || setup.screen_recording_granted),
  );
  const permissionPending = Boolean(
    setup?.screen_recording_required && !setup.screen_recording_granted,
  );
  const shouldOfferRestart = Boolean(
    permissionPending && setup?.screen_recording_requested_this_launch,
  );
  const switchStillOff = Boolean(
    permissionPending
      && setup
      && !setup.screen_recording_can_request
      && !setup.screen_recording_requested_this_launch,
  );
  const permissionStatus = screenReady
    ? (setup?.screen_recording_required ? "Allowed" : "Ready")
    : shouldOfferRestart
      ? "Restart required"
      : switchStillOff
        ? "Still off"
        : "Needs approval";
  return (
    <main className="onboarding-shell">
      <section className="onboarding-intro" aria-label="Captures">
        <div className="onboarding-brand">
          <span className="onboarding-brand-icon"><CaptureSetupIcon /></span>
          <span>Captures</span>
        </div>
        <div className="onboarding-visual" aria-hidden="true">
          <span className="onboarding-frame onboarding-frame-large" />
          <span className="onboarding-frame onboarding-frame-small" />
          <span className="onboarding-orbit onboarding-orbit-accent" />
          <span className="onboarding-orbit onboarding-orbit-blue" />
          <span className="onboarding-spark"><CaptureSetupIcon /></span>
        </div>
      </section>

      <section className="onboarding-setup" aria-labelledby="onboarding-setup-title">
        <header className="onboarding-setup-header">
          <h1 id="onboarding-setup-title">{setupTitle(setup?.platform)}</h1>
          <p>{setupDescription(setup?.platform)}</p>
        </header>

        <div className="onboarding-permissions" aria-live="polite">
          <article className={`onboarding-permission${screenReady ? " ready" : " action-required"}`}>
            <span className="onboarding-permission-icon screen"><ScreenAccessIcon /></span>
            <div className="onboarding-permission-copy">
              <div className="onboarding-permission-title">
                <h3>Screen capture</h3>
                <span className={setup?.screen_recording_required ? "required" : undefined}>
                  {setup?.screen_recording_required ? "Required" : "Built in"}
                </span>
              </div>
              <p>{setup ? screenAccessDescription(setup.platform, setup.screen_recording_required, shouldOfferRestart, switchStillOff) : "Checking the access available on this computer…"}</p>
              {setup && (
                <span className={`onboarding-permission-status${screenReady ? " ready" : ""}`}>
                  <i aria-hidden="true" /> {permissionStatus}
                </span>
              )}
            </div>
            {permissionPending && setup && (
              <div className="onboarding-permission-actions">
                <button
                  type="button"
                  className="onboarding-permission-button"
                  disabled={busy !== null}
                  onClick={() => void requestPermission()}
                >
                  {busy === "permission"
                    ? "Opening…"
                    : setup.screen_recording_can_request
                      ? "Allow access"
                      : "Open Settings"}
                </button>
              </div>
            )}
          </article>

          {setup && setup.platform === "macos" && (
            <article className={`onboarding-permission optional${setup.microphone_granted ? " ready" : ""}`}>
              <span className="onboarding-permission-icon microphone"><MicrophoneAccessIcon /></span>
              <div className="onboarding-permission-copy">
                <div className="onboarding-permission-title">
                  <h3>Microphone</h3>
                  <span className="optional">Optional</span>
                </div>
                <p>{microphoneDescription(setup.microphone_granted)}</p>
                <span className={`onboarding-permission-status${setup.microphone_granted ? " ready" : " optional"}`}>
                  <i aria-hidden="true" /> {microphoneStatus(setup, microphoneAsked)}
                </span>
              </div>
              {!setup.microphone_granted && (
                <div className="onboarding-permission-actions">
                  <button
                    type="button"
                    className="onboarding-permission-button"
                    disabled={busy !== null}
                    onClick={() => void requestMicrophone()}
                  >
                    {busy === "microphone"
                      ? "Opening…"
                      : setup.microphone_can_request
                        ? "Allow microphone"
                        : "Microphone Settings"}
                  </button>
                </div>
              )}
            </article>
          )}
        </div>

        {error && <p className="onboarding-error" role="alert">{error}</p>}

        <footer className="onboarding-footer">
          <div>
            {shouldOfferRestart ? (
              <button
                type="button"
                className="onboarding-primary-button"
                disabled={busy !== null}
                onClick={restart}
              >
                {busy === "restart" ? "Restarting…" : "Restart Captures"}
              </button>
            ) : (
              <button
                type="button"
                className="onboarding-primary-button"
                disabled={!screenReady || busy !== null}
                onClick={() => void complete()}
              >
                {busy === "complete" ? "Starting…" : "Start capturing"}
                {busy !== "complete" && <ArrowIcon />}
              </button>
            )}
          </div>
        </footer>
      </section>
    </main>
  );
}
