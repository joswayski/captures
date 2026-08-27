import { invoke, isTauri } from "./lib/tauri";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState } from "react";

import type { OnboardingState } from "./types";

type BusyAction = "permission" | "microphone" | "restart" | "complete" | null;

const PERMISSION_POLL_MS = 1_500;
const SETTINGS_AWAY_MS = 2_500;

function ScreenAccessIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <rect x="3" y="4" width="18" height="13" rx="2.5" />
      <path d="M8 21h8M12 17v4" />
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

function CheckmarkIcon() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M3.2 8.2 6.4 11.4 12.8 4.6" />
    </svg>
  );
}

function setupTitle(platform: string | undefined) {
  if (platform === "macos") return "Required permissions";
  return "You’re ready to capture";
}

function screenAccessDescription(platform: string, required: boolean, restartRequired: boolean, stillOff: boolean) {
  if (platform === "macos" && stillOff) {
    return "The switch for this copy of Captures is still off. A local build is a different row from a downloaded app. Turn it on, then restart.";
  }
  if (platform === "macos" && restartRequired) {
    return "Turn the switch on next to this copy of Captures, then restart. A local build is a different row from a downloaded app. macOS does not apply the permission until Captures relaunches.";
  }
  if (platform === "macos") {
    return "This allows Captures to read the pixels you choose to capture. macOS keeps everything else hidden.";
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

function microphoneDescription(granted: boolean, asked: boolean) {
  if (granted) {
    return "macOS will not ask again. Turn the microphone on when you start a recording.";
  }
  if (asked) {
    return "Turn Captures on in Microphone settings. macOS only lists apps after they ask.";
  }
  return "Allow it now so a recording does not pause to ask, or wait until you pick a mic.";
}

function permissionActionLabel(setup: OnboardingState, busy: boolean) {
  if (busy) return "Opening…";
  return setup.screen_recording_can_request ? "Allow access" : "Open Settings";
}

function microphoneActionLabel(setup: OnboardingState, busy: boolean, asked: boolean) {
  if (busy) return "Opening…";
  if (asked && !setup.microphone_can_request) return "Open Settings";
  return "Allow microphone";
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
  const showMicrophone = setup?.platform === "macos";
  const primaryLabel = shouldOfferRestart
    ? (busy === "restart" ? "Restarting…" : "Restart Captures")
    : (busy === "complete" ? "Finishing…" : "Start capturing");

  return (
    <main className="onboarding-shell">
      <div className="onboarding-stage">
        <header className="onboarding-copy" aria-labelledby="onboarding-setup-title">
          <p className="onboarding-kicker">Captures</p>
          <h1 id="onboarding-setup-title">{setupTitle(setup?.platform)}</h1>
        </header>

        <div className="onboarding-panel">
          <div className="onboarding-permissions" aria-live="polite">
            <article className="onboarding-permission">
              <span className="onboarding-permission-icon"><ScreenAccessIcon /></span>
              <div className="onboarding-permission-copy">
                <h3>Screen capture</h3>
                <p>
                  {setup
                    ? screenAccessDescription(
                        setup.platform,
                        setup.screen_recording_required,
                        shouldOfferRestart,
                        switchStillOff,
                      )
                    : "Checking the access available on this computer…"}
                </p>
              </div>
              {setup && (
                <ScreenPermissionAction
                  setup={setup}
                  screenReady={screenReady}
                  shouldOfferRestart={shouldOfferRestart}
                  switchStillOff={switchStillOff}
                  busy={busy !== null}
                  opening={busy === "permission"}
                  onRequest={() => void requestPermission()}
                />
              )}
            </article>

            {showMicrophone && setup && (
              <article className="onboarding-permission">
                <span className="onboarding-permission-icon"><MicrophoneAccessIcon /></span>
                <div className="onboarding-permission-copy">
                  <div className="onboarding-permission-heading">
                    <h3>Microphone</h3>
                    <span className="optional">Optional</span>
                  </div>
                  <p>{microphoneDescription(setup.microphone_granted, microphoneAsked)}</p>
                </div>
                {setup.microphone_granted ? (
                  <GrantedStatus label="Granted" />
                ) : (
                  <div className="onboarding-permission-actions">
                    <button
                      type="button"
                      className="onboarding-permission-button"
                      disabled={busy !== null}
                      onClick={() => void requestMicrophone()}
                    >
                      {microphoneActionLabel(setup, busy === "microphone", microphoneAsked)}
                    </button>
                  </div>
                )}
              </article>
            )}
          </div>

          {error && <p className="onboarding-error" role="alert">{error}</p>}

          <div className="onboarding-actions">
            {shouldOfferRestart ? (
              <button
                type="button"
                className="onboarding-primary-button"
                disabled={busy !== null}
                onClick={restart}
              >
                {primaryLabel}
              </button>
            ) : (
              <button
                type="button"
                className={`onboarding-primary-button${screenReady && busy === null ? " cta-pulse" : ""}`}
                disabled={!screenReady || busy !== null}
                onClick={() => void complete()}
              >
                {primaryLabel}
              </button>
            )}
          </div>
        </div>
      </div>
    </main>
  );
}

function GrantedStatus({ label }: { label: string }) {
  return (
    <span className="onboarding-permission-status ready">
      {label} <CheckmarkIcon />
    </span>
  );
}

function ScreenPermissionAction({
  setup,
  screenReady,
  shouldOfferRestart,
  switchStillOff,
  busy,
  opening,
  onRequest,
}: {
  setup: OnboardingState;
  screenReady: boolean;
  shouldOfferRestart: boolean;
  switchStillOff: boolean;
  busy: boolean;
  opening: boolean;
  onRequest: () => void;
}) {
  if (screenReady) {
    return <GrantedStatus label={setup.screen_recording_required ? "Granted" : "Ready"} />;
  }

  return (
    <div className="onboarding-permission-actions">
      {shouldOfferRestart && (
        <span className="onboarding-permission-status">Restart required</span>
      )}
      {switchStillOff && (
        <span className="onboarding-permission-status">Still off</span>
      )}
      <button
        type="button"
        className="onboarding-permission-button"
        disabled={busy}
        onClick={onRequest}
      >
        {permissionActionLabel(setup, opening)}
      </button>
    </div>
  );
}
