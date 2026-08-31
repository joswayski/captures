import { createFileRoute } from "@tanstack/react-router";
import { type FormEvent, useEffect, useState } from "react";
import { apiJson } from "../sharingClient";

export const Route = createFileRoute("/login")({
  head: () => ({ meta: [{ title: "Sign in · Captures" }] }),
  component: LoginPage,
});

function LoginPage() {
  const [email, setEmail] = useState("");
  const [code, setCode] = useState("");
  const [stage, setStage] = useState<"email" | "code">("email");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [googleEnabled, setGoogleEnabled] = useState(false);

  useEffect(() => {
    let active = true;
    void apiJson<{ google: boolean }>("/api/auth/providers")
      .then(({ google }) => { if (active) setGoogleEnabled(google); })
      .catch(() => undefined);
    return () => { active = false; };
  }, []);

  async function submitEmail(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await apiJson("/api/auth/email/start", {
        method: "POST",
        body: JSON.stringify({ email, client: "web" }),
      });
      setStage("code");
    } catch (error) {
      setError(error instanceof Error ? error.message : "Couldn’t send a sign-in code");
    } finally {
      setBusy(false);
    }
  }

  async function submitCode(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await apiJson("/api/auth/email/verify", {
        method: "POST",
        body: JSON.stringify({ email, code, client: "web" }),
      });
      window.location.assign("/library");
    } catch (error) {
      setError(error instanceof Error ? error.message : "That code didn’t work");
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="account-shell">
      <section className="account-card" aria-labelledby="login-title">
        <a href="/" className="account-brand">Captures</a>
        <p className="eyebrow">Private beta</p>
        <h1 id="login-title">Sign in to your library</h1>
        <p className="account-copy">
          Your remote captures stay private until you explicitly create a shared link.
        </p>

        {stage === "email" ? (
          <form onSubmit={submitEmail} className="account-form">
            <label htmlFor="email">Email</label>
            <input
              id="email"
              type="email"
              autoComplete="email"
              required
              value={email}
              onChange={(event) => setEmail(event.target.value)}
            />
            <button type="submit" disabled={busy}>{busy ? "Sending…" : "Email me a code"}</button>
          </form>
        ) : (
          <form onSubmit={submitCode} className="account-form">
            <label htmlFor="code">Six-digit code sent to {email}</label>
            <input
              id="code"
              inputMode="numeric"
              autoComplete="one-time-code"
              pattern="[0-9]{6}"
              maxLength={6}
              required
              value={code}
              onChange={(event) => setCode(event.target.value.replace(/\D/gu, ""))}
            />
            <button type="submit" disabled={busy}>{busy ? "Signing in…" : "Sign in"}</button>
            <button type="button" className="account-secondary" onClick={() => setStage("email")}>
              Use another email
            </button>
          </form>
        )}
        {error && <p className="account-error" role="alert">{error}</p>}
        {googleEnabled && (
          <>
            <div className="account-divider"><span>or</span></div>
            <a className="account-google" href="/api/auth/google/start">Continue with Google</a>
          </>
        )}
      </section>
    </main>
  );
}
