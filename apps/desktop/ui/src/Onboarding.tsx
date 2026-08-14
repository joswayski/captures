import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useState } from "react";

import type { OnboardingState } from "./types";

type BusyAction = "permission" | "refresh" | "restart" | "complete" | null;

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

function AudioAccessIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M4 13v-2M8 16V8M12 19V5M16 16V8M20 13v-2" />
    </svg>
  );
}

function ArrowIcon() {
  return <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M4 10h11M11 6l4 4-4 4" /></svg>;
}

function screenAccessDescription(platform: string, required: boolean) {
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

export function Onboarding() {
  const [setup, setSetup] = useState<OnboardingState | null>(null);
  const [busy, setBusy] = useState<BusyAction>(null);
  const [error, setError] = useState("");

  const refresh = useCallback(async (showBusy = false) => {
    if (showBusy) setBusy("refresh");
    setError("");
    try {
      setSetup(await invoke<OnboardingState>("get_onboarding_state"));
    } catch (refreshError) {
      setError(`Couldn’t check screen access: ${String(refreshError)}`);
    } finally {
      if (showBusy) setBusy(null);
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
    void load();
    window.addEventListener("focus", handleFocus);
    return () => {
      active = false;
      window.removeEventListener("focus", handleFocus);
    };
  }, []);

  const requestPermission = async () => {
    setBusy("permission");
    setError("");
    try {
      setSetup(await invoke<OnboardingState>("request_onboarding_screen_permission"));
    } catch (permissionError) {
      setError(`Couldn’t open screen access: ${String(permissionError)}`);
    } finally {
      setBusy(null);
    }
  };

  const restart = () => {
    setBusy("restart");
    setError("");
    void invoke("restart_captures_for_permissions").catch((restartError) => {
      setBusy(null);
      setError(`Couldn’t restart Captures: ${String(restartError)}`);
    });
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
    permissionPending
      && setup
      && (!setup.screen_recording_can_request || setup.screen_recording_requested_this_launch),
  );
  const permissionStatus = screenReady
    ? (setup?.screen_recording_required ? "Allowed" : "Ready")
    : setup?.screen_recording_requested_this_launch
      ? "Waiting for macOS"
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
          <h1 id="onboarding-setup-title">One place for access</h1>
          <p>Captures asks only for what a feature needs, when it needs it.</p>
        </header>

        <div className="onboarding-permissions" aria-live="polite">
          <article className={`onboarding-permission${screenReady ? " ready" : " action-required"}`}>
            <span className="onboarding-permission-icon screen"><ScreenAccessIcon /></span>
            <div className="onboarding-permission-copy">
              <div className="onboarding-permission-title">
                <h3>Screen capture</h3>
                <span>{setup?.screen_recording_required ? "Required" : "Built in"}</span>
              </div>
              <p>{setup ? screenAccessDescription(setup.platform, setup.screen_recording_required) : "Checking the access available on this computer…"}</p>
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
                {!setup.screen_recording_can_request && (
                  <button
                    type="button"
                    className="onboarding-text-button"
                    disabled={busy !== null}
                    onClick={() => void refresh(true)}
                  >
                    {busy === "refresh" ? "Checking…" : "Check again"}
                  </button>
                )}
              </div>
            )}
          </article>

          <article className="onboarding-permission optional">
            <span className="onboarding-permission-icon audio"><AudioAccessIcon /></span>
            <div className="onboarding-permission-copy">
              <div className="onboarding-permission-title">
                <h3>Recording audio</h3>
                <span>Optional</span>
              </div>
              <p>Microphone and desktop-audio access are requested only when you turn those sources on for a recording.</p>
              <span className="onboarding-permission-status optional"><i aria-hidden="true" /> On demand</span>
            </div>
          </article>
        </div>

        {error && <p className="onboarding-error" role="alert">{error}</p>}

        <footer className="onboarding-footer">
          <div>
            {shouldOfferRestart && (
              <button
                type="button"
                className="onboarding-secondary-button"
                disabled={busy !== null}
                onClick={restart}
              >
                {busy === "restart" ? "Restarting…" : "Restart Captures"}
              </button>
            )}
            <button
              type="button"
              className="onboarding-primary-button"
              disabled={!screenReady || busy !== null}
              onClick={() => void complete()}
            >
              {busy === "complete" ? "Starting…" : "Start capturing"}
              {busy !== "complete" && <ArrowIcon />}
            </button>
          </div>
        </footer>
      </section>
    </main>
  );
}
