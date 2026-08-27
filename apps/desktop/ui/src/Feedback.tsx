import { invoke } from "./lib/tauri";
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
      <header className="feedback-header">
        <div>
          <span className="eyebrow">Captures</span>
          <h1>Send feedback</h1>
        </div>
      </header>

      <p className="help-text feedback-intro">
        Tell us what broke, what is missing, or what you wish worked better. Captures only sends
        what you type here plus app and system details listed below — never your screenshots or
        recordings.
      </p>

      <section className="settings-section feedback-section">
        <h2>Category</h2>
        <div className="feedback-categories" role="radiogroup" aria-label="Feedback category">
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
      </section>

      <section className="settings-section feedback-section">
        <label className="field-label" htmlFor="feedback-message">
          Message
        </label>
        <textarea
          id="feedback-message"
          className="feedback-message"
          value={message}
          onChange={(event) => {
            setMessage(event.target.value);
            if (status === "sent" || status === "error") setStatus("idle");
          }}
          placeholder="What happened? What did you expect?"
          rows={7}
          maxLength={8_000}
          disabled={status === "sending"}
        />
      </section>

      <section className="settings-section feedback-section">
        <label className="field-label" htmlFor="feedback-contact">
          Contact <span className="feedback-optional">(optional)</span>
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
        <p className="help-text">Only if you want a reply. Leave blank to stay anonymous.</p>
      </section>

      <section className="settings-section feedback-section">
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

      <div className="feedback-actions">
        <button
          type="button"
          className="feedback-submit"
          disabled={!canSubmit}
          onClick={() => void submit()}
        >
          {status === "sending" ? "Sending…" : "Send feedback"}
        </button>
      </div>
    </main>
  );
}
