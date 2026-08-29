import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";

export type FeedbackCategory = "bug" | "idea" | "other";

export interface FeedbackContext {
  app_version: string;
  os: string;
  os_version: string;
  arch: string;
}

export interface FeedbackSubmitResult {
  ok: boolean;
}

const CATEGORIES: Array<{ id: FeedbackCategory; label: string; description: string }> = [
  { id: "bug", label: "Bug", description: "Something is broken or unexpected" },
  { id: "idea", label: "Idea", description: "A feature or improvement" },
  { id: "other", label: "Other", description: "Anything else" },
];

const MESSAGE_PLACEHOLDERS: Record<FeedbackCategory, string> = {
  bug: "What happened? What did you expect?",
  idea: "What's the idea? What problem would it solve?",
  other: "What would you like us to know?",
};

function formatOsLabel(context: FeedbackContext): string {
  const parts = [context.os, context.os_version, context.arch].filter(Boolean);
  return parts.join(" · ");
}

export function Feedback() {
  const [context, setContext] = useState<FeedbackContext | null>(null);
  const [category, setCategory] = useState<FeedbackCategory>("bug");
  const [message, setMessage] = useState("");
  const [contact, setContact] = useState("");
  const [status, setStatus] = useState<"idle" | "sending" | "sent" | "error">("idle");
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    void invoke<FeedbackContext>("get_feedback_context")
      .then((loaded) => {
        if (active) setContext(loaded);
      })
      .catch(() => {
        if (active) {
          setContext({
            app_version: "unknown",
            os: "unknown",
            os_version: "",
            arch: "",
          });
        }
      });
    return () => {
      active = false;
    };
  }, []);

  const canSubmit = useMemo(
    () => message.trim().length > 0 && status !== "sending",
    [message, status],
  );

  const messagePlaceholder = MESSAGE_PLACEHOLDERS[category];

  const submit = async () => {
    if (!canSubmit) return;
    setStatus("sending");
    setError("");
    try {
      await invoke<FeedbackSubmitResult>("submit_feedback", {
        draft: {
          message: message.trim(),
          contact: contact.trim() || null,
          category,
        },
      });
      setStatus("sent");
      setMessage("");
    } catch (submitError) {
      setStatus("error");
      setError(String(submitError));
    }
  };

  return (
    <main className="feedback">
      <div className="feedback-shell">
        <header className="feedback-header">
          <div>
            <span className="eyebrow">Captures</span>
            <h1>Send feedback</h1>
            <p className="help-text feedback-intro">
              Tell us what broke, what is missing, or what you wish worked better. Captures sends
              what you type here plus the app and system details listed below.
            </p>
          </div>
        </header>

        <section className="settings-card feedback-section">
          <div className="feedback-field">
            <span className="field-label" id="feedback-category-label">Category</span>
            <div
              className="feedback-categories"
              role="radiogroup"
              aria-labelledby="feedback-category-label"
              aria-label="Feedback category"
            >
              {CATEGORIES.map((item) => (
                <button
                  key={item.id}
                  type="button"
                  className={`feedback-category${category === item.id ? " active" : ""}`}
                  role="radio"
                  aria-checked={category === item.id}
                  onClick={() => setCategory(item.id)}
                >
                  <strong>{item.label}</strong>
                  <small>{item.description}</small>
                </button>
              ))}
            </div>
          </div>

          <div className="feedback-field">
            <label className="field-label" htmlFor="feedback-message">Message</label>
            <textarea
              id="feedback-message"
              className="feedback-message"
              value={message}
              onChange={(event) => {
                setMessage(event.target.value);
                if (status === "sent" || status === "error") setStatus("idle");
              }}
              placeholder={messagePlaceholder}
              rows={7}
              maxLength={8_000}
              disabled={status === "sending"}
            />
          </div>

          <div className="feedback-field">
            <label className="field-label" htmlFor="feedback-contact">
              Contact <span className="feedback-optional">optional</span>
            </label>
            <input
              id="feedback-contact"
              className="feedback-contact"
              value={contact}
              onChange={(event) => setContact(event.target.value)}
              placeholder="X handle, GitHub username, email…"
              maxLength={200}
              disabled={status === "sending"}
              autoComplete="off"
              spellCheck={false}
            />
            <p className="help-text">
              Optional — we may use this if we need to ask a follow-up question.
            </p>
          </div>
        </section>

        <section className="settings-card feedback-section feedback-meta-card">
          <h2>Included automatically</h2>
          <dl className="feedback-meta">
            <div>
              <dt>App version</dt>
              <dd>{context?.app_version ?? "…"}</dd>
            </div>
            <div>
              <dt>System</dt>
              <dd>{context ? formatOsLabel(context) : "…"}</dd>
            </div>
          </dl>
        </section>

        <footer className="feedback-actions">
          {status === "error" && error && (
            <p className="feedback-status feedback-status-error" role="alert">
              {error}
            </p>
          )}
          {status === "sent" && (
            <p className="feedback-status feedback-status-ok" role="status">
              Thanks — feedback sent.
            </p>
          )}
          <button
            type="button"
            className="feedback-submit"
            disabled={!canSubmit}
            onClick={() => void submit()}
          >
            {status === "sending" ? "Sending…" : "Send feedback"}
          </button>
        </footer>
      </div>
    </main>
  );
}
