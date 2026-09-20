import { createFileRoute } from "@tanstack/react-router";
import { Share2 } from "lucide-react";
import { FormEvent, useCallback, useEffect, useState } from "react";
import { formatBytes, validateShare, validateUpload } from "../accountModel";

type User = { id: string; email: string };
type Share = {
  id: string;
  visibility: "private" | "unlisted" | "public";
  passwordProtected: boolean;
  expiresAt: string | null;
  revokedAt: string | null;
};
type Upload = {
  id: string;
  contentType: string;
  byteSize: number;
  createdAt: string;
  shares: Share[];
};

async function api<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, { credentials: "same-origin", ...init });
  if (!response.ok) {
    const body = (await response.json().catch(() => null)) as {
      error?: string;
    } | null;
    throw Object.assign(
      new Error(body?.error || `Request failed (${response.status})`),
      { status: response.status, body },
    );
  }
  return (response.status === 204 ? undefined : await response.json()) as T;
}

export const Route = createFileRoute("/account")({
  head: () => ({
    meta: [
      { title: "Account — Captures" },
      { name: "robots", content: "noindex, nofollow" },
      { name: "referrer", content: "no-referrer" },
    ],
  }),
  component: AccountPage,
});

function AccountPage() {
  const [user, setUser] = useState<User | null>(null);
  const [loading, setLoading] = useState(true);
  const [disabled, setDisabled] = useState(false);
  const [accountError, setAccountError] = useState(false);
  const [logoutError, setLogoutError] = useState("");
  const loadMe = useCallback(async () => {
    try {
      setUser((await api<{ user: User }>("/api/account/me")).user);
    } catch (error) {
      const status = (error as { status?: number }).status;
      if (status === 503) setDisabled(true);
      else if (status !== 401) setAccountError(true);
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void loadMe();
  }, [loadMe]);

  return (
    <main className="account-shell">
      <header className="account-header">
        <a href="/" className="account-brand">
          Captures
        </a>
        {user && (
          <button
            className="text-button"
            onClick={async () => {
              try {
                await api("/api/auth/logout", { method: "POST" });
                setUser(null);
                setLogoutError("");
              } catch {
                setLogoutError("Could not sign out. Please try again.");
              }
            }}
          >
            Sign out
          </button>
        )}
      </header>
      {logoutError && (
        <p role="alert" className="error-box">
          {logoutError}
        </p>
      )}
      {loading ? (
        <Status title="Loading your account…" />
      ) : disabled ? (
        <Status
          title="Accounts are unavailable"
          copy="Account services are disabled right now. Your local captures are unaffected."
        />
      ) : accountError ? (
        <Status
          title="Unable to load your account"
          copy="The account service is temporarily unavailable. Try again later."
        />
      ) : user ? (
        <Library user={user} />
      ) : (
        <Login onAuthenticated={setUser} />
      )}
    </main>
  );
}

function Login({ onAuthenticated }: { onAuthenticated: (user: User) => void }) {
  const [email, setEmail] = useState("");
  const [challengeId, setChallengeId] = useState("");
  const [code, setCode] = useState("");
  const [error, setError] = useState("");
  const [remaining, setRemaining] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  async function request(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const result = await api<{ challengeId: string }>(
        "/api/auth/email/request",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ email }),
        },
      );
      setChallengeId(result.challengeId);
      setCode("");
      setRemaining(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function verify(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const result = await api<{ user: User }>("/api/auth/email/verify", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ challengeId, code, transport: "cookie" }),
      });
      onAuthenticated(result.user);
    } catch (e) {
      const value = e as Error & { body?: { attemptsRemaining?: number } };
      setRemaining(value.body?.attemptsRemaining ?? null);
      setError(value.message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="auth-card">
      <p className="eyebrow">Your account</p>
      <h1>{challengeId ? "Check your email" : "Keep your images together."}</h1>
      <p className="lede">
        {challengeId ? (
          <>
            Enter the six-character code sent to <strong>{email}</strong>.
          </>
        ) : (
          "Sign in or create an account with a one-time email code. No password to remember."
        )}
      </p>
      {!challengeId ? (
        <form onSubmit={request} className="form-stack">
          <label>
            Email address
            <input
              type="email"
              autoComplete="email"
              required
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="you@example.com"
            />
          </label>
          <button className="primary-button" disabled={busy}>
            {busy ? "Sending…" : "Email me a code"}
          </button>
        </form>
      ) : (
        <form onSubmit={verify} className="form-stack">
          <label>
            Verification code
            <input
              className="code-input"
              required
              minLength={6}
              maxLength={6}
              pattern="[A-Za-z0-9]{6}"
              autoComplete="one-time-code"
              autoFocus
              value={code}
              onChange={(e) =>
                setCode(e.target.value.toUpperCase().replace(/[^A-Z0-9]/g, ""))
              }
              placeholder="ABC123"
            />
          </label>
          {remaining === 1 && <p className="warning">One attempt remaining.</p>}
          {remaining === 0 && (
            <p className="warning">
              That code can no longer be used. Request a new one.
            </p>
          )}
          <button
            className="primary-button"
            disabled={busy || code.length !== 6 || remaining === 0}
          >
            {busy ? "Checking…" : "Continue"}
          </button>
          <button
            type="button"
            className="text-button"
            disabled={busy}
            onClick={() => {
              setChallengeId("");
              setError("");
            }}
          >
            Use another email or resend
          </button>
        </form>
      )}
      {error && (
        <p role="alert" className="error-box">
          {error}
        </p>
      )}
      <p className="privacy-note">
        Local captures never require an account. Nothing uploads automatically.
      </p>
    </section>
  );
}

function Library({ user }: { user: User }) {
  const [uploads, setUploads] = useState<Upload[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const refresh = useCallback(async () => {
    try {
      setUploads((await api<{ uploads: Upload[] }>("/api/uploads")).uploads);
      setError("");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void refresh();
  }, [refresh]);
  async function upload(file?: File) {
    if (!file) return;
    const problem = validateUpload(file);
    if (problem) return setError(problem);
    setBusy(true);
    setError("");
    try {
      await api("/api/uploads", {
        method: "POST",
        headers: { "Content-Type": file.type },
        body: file,
      });
      await refresh();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <section className="library-heading">
        <div>
          <p className="eyebrow">Signed in as {user.email}</p>
          <h1>Your images</h1>
          <p className="lede">
            Upload a static PNG, JPEG, or WebP up to 20 MiB, then choose exactly
            how it can be shared. Limit: 100 images or 1 GiB per account.
          </p>
        </div>
        <label
          className={`primary-button upload-button ${busy ? "is-disabled" : ""}`}
        >
          {busy ? "Uploading…" : "Upload image"}
          <input
            type="file"
            accept="image/png,image/jpeg,image/webp"
            disabled={busy}
            onChange={(e) => {
              void upload(e.target.files?.[0]);
              e.target.value = "";
            }}
          />
        </label>
      </section>
      {error && (
        <p role="alert" className="error-box">
          {error}
        </p>
      )}
      {loading ? (
        <p role="status">Loading images…</p>
      ) : uploads.length === 0 && !error ? (
        <div className="empty-state">
          <h2>No images yet</h2>
          <p>
            Your uploaded images will appear here. Files stay private until you
            create a share link.
          </p>
        </div>
      ) : (
        <div className="upload-grid">
          {uploads.map((item) => (
            <UploadCard
              key={item.id}
              upload={item}
              refresh={refresh}
              setError={setError}
            />
          ))}
        </div>
      )}
    </>
  );
}

function UploadCard({
  upload,
  refresh,
  setError,
}: {
  upload: Upload;
  refresh: () => Promise<void>;
  setError: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [visibility, setVisibility] = useState<Share["visibility"]>("unlisted");
  const [password, setPassword] = useState("");
  const [expiresAt, setExpiresAt] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState("");
  async function createShare(event: FormEvent) {
    event.preventDefault();
    const problem = validateShare(password, expiresAt);
    if (problem) return setError(problem);
    setBusy(true);
    setError("");
    try {
      await api(`/api/uploads/${upload.id}/shares`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          visibility,
          ...(password ? { password } : {}),
          expiresAt: expiresAt ? new Date(expiresAt).toISOString() : null,
        }),
      });
      setOpen(false);
      setPassword("");
      await refresh();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <article className="upload-card">
      <img
        src={`/api/uploads/${upload.id}/media`}
        alt="Uploaded capture"
        loading="lazy"
      />
      <div className="upload-meta">
        <span>{formatBytes(upload.byteSize)}</span>
        <time>{new Date(upload.createdAt).toLocaleDateString()}</time>
      </div>
      <div className="share-list">
        {upload.shares.map((share) => (
          <div className="share-row" key={share.id}>
            <div>
              <span className="visibility">{share.visibility}</span>
              {share.passwordProtected && <span> · password</span>}
              {share.revokedAt && <span> · revoked</span>}
            </div>
            {!share.revokedAt && (
              <div>
                <button
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(
                        new URL(`/s/${share.id}`, window.location.origin).href,
                      );
                      setCopied(share.id);
                    } catch {
                      setError(
                        "Copy failed. Open the link and copy it from your address bar.",
                      );
                    }
                  }}
                >
                  {copied === share.id ? "Copied" : "Copy link"}
                </button>
                <a href={`/s/${share.id}`} target="_blank" rel="noreferrer">
                  Open
                </a>
                <button
                  onClick={async () => {
                    if (
                      !confirm(
                        "Revoke this link? Future requests will stop immediately, but existing downloads cannot be recalled.",
                      )
                    )
                      return;
                    try {
                      await api(`/api/shares/${share.id}/revoke`, {
                        method: "POST",
                      });
                      await refresh();
                    } catch (e) {
                      setError((e as Error).message);
                    }
                  }}
                >
                  Revoke
                </button>
              </div>
            )}
          </div>
        ))}
      </div>
      {open ? (
        <form className="share-form" onSubmit={createShare}>
          <label>
            Access
            <select
              value={visibility}
              onChange={(e) =>
                setVisibility(e.target.value as Share["visibility"])
              }
            >
              <option value="unlisted">Unlisted — anyone with link</option>
              <option value="public">Public — may be indexed</option>
              <option value="private">Private — only you</option>
            </select>
          </label>
          <label>
            Optional password
            <input
              type="password"
              minLength={8}
              maxLength={128}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="At least 8 characters"
            />
          </label>
          <label>
            Optional expiry
            <input
              type="datetime-local"
              value={expiresAt}
              onChange={(e) => setExpiresAt(e.target.value)}
            />
          </label>
          <div className="button-row">
            <button className="primary-button" disabled={busy}>
              {busy ? "Creating…" : "Create link"}
            </button>
            <button
              type="button"
              className="text-button"
              disabled={busy}
              onClick={() => setOpen(false)}
            >
              Cancel
            </button>
          </div>
        </form>
      ) : (
        <div className="card-actions">
          <button
            className="secondary-button"
            aria-label="Share"
            title="Share"
            onClick={() => setOpen(true)}
          >
            <Share2 size={18} aria-hidden="true" />
          </button>
          <button
            className="danger-button"
            onClick={async () => {
              if (
                !confirm(
                  "Delete this image and all its share links? This cannot be undone.",
                )
              )
                return;
              try {
                await api(`/api/uploads/${upload.id}`, { method: "DELETE" });
                await refresh();
              } catch (e) {
                setError((e as Error).message);
              }
            }}
          >
            Delete
          </button>
        </div>
      )}
    </article>
  );
}

function Status({ title, copy }: { title: string; copy?: string }) {
  return (
    <section className="auth-card">
      <h1>{title}</h1>
      {copy && <p className="lede">{copy}</p>}
    </section>
  );
}
