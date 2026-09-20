import { createFileRoute } from "@tanstack/react-router";
import { Link2Off, Share2, Trash2, Undo2 } from "lucide-react";
import { FormEvent, useCallback, useEffect, useRef, useState } from "react";
import {
  assetMediaKind,
  formatBytes,
  uploadAsset,
  validateShare,
  validateUpload,
  type Asset,
} from "../accountModel";

type User = { id: string; email: string };
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
  const [assets, setAssets] = useState<Asset[]>([]);
  const [trash, setTrash] = useState(false);
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState("");
  const [loading, setLoading] = useState(true);
  const uploadController = useRef<AbortController | null>(null);
  const refresh = useCallback(async () => {
    setRevision((value) => value + 1);
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    setLoading(true);
    void api<{ assets: Asset[] }>(
      trash ? "/api/assets?deleted=true" : "/api/assets",
      { signal: controller.signal },
    )
      .then(({ assets }) => {
        if (!controller.signal.aborted) {
          setAssets(assets);
          setError("");
        }
      })
      .catch((e: Error) => {
        if (!controller.signal.aborted) setError(e.message);
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [trash, revision]);
  async function upload(file?: File) {
    if (!file) return;
    const problem = validateUpload(file);
    if (problem) return setError(problem);
    setBusy(true);
    setError("");
    setProgress("Preparing upload…");
    const controller = new AbortController();
    uploadController.current = controller;
    try {
      const asset = await uploadAsset(file, {
        signal: controller.signal,
        onProgress: (complete, total) =>
          setProgress(`Uploading part ${complete} of ${total}…`),
      });
      setAssets((current) => [
        asset,
        ...current.filter((item) => item.id !== asset.id),
      ]);
      setProgress("");
    } catch (e) {
      setProgress("");
      setError(
        controller.signal.aborted ? "Upload cancelled." : (e as Error).message,
      );
    } finally {
      uploadController.current = null;
      setBusy(false);
    }
  }
  return (
    <>
      <section className="library-heading">
        <div>
          <p className="account-identity">Signed in as {user.email}</p>
          <h1>{trash ? "Trash" : "Your captures"}</h1>
          <p className="lede">
            {trash
              ? "Your files are kept until you restore them. Old share links stay disabled."
              : "Upload screenshots, GIFs, videos, or other files. Captures stay private until you share them."}
          </p>
        </div>
        {!trash && (
          <div className="upload-controls">
            <label
              className={`primary-button upload-button ${busy ? "is-disabled" : ""}`}
            >
              {busy ? progress || "Uploading…" : "Upload capture"}
              <input
                type="file"
                disabled={busy}
                onChange={(e) => {
                  void upload(e.target.files?.[0]);
                  e.target.value = "";
                }}
              />
            </label>
            {busy && (
              <button
                className="text-button"
                onClick={() => uploadController.current?.abort()}
              >
                Cancel upload
              </button>
            )}
          </div>
        )}
      </section>
      <nav className="library-tabs" aria-label="Capture library">
        <button
          className="secondary-button"
          aria-pressed={!trash}
          disabled={busy}
          onClick={() => setTrash(false)}
        >
          Captures
        </button>
        <button
          className="secondary-button"
          aria-pressed={trash}
          disabled={busy}
          onClick={() => setTrash(true)}
        >
          Trash
        </button>
      </nav>
      {error && (
        <p role="alert" className="error-box">
          {error}
        </p>
      )}
      {loading ? (
        <p role="status">Loading captures…</p>
      ) : assets.length === 0 && !error ? (
        <div className="empty-state">
          <h2>{trash ? "Trash is empty" : "No captures yet"}</h2>
          <p>
            {trash
              ? "Deleted captures will appear here. Nothing is permanently removed."
              : "Your uploaded captures will appear here."}
          </p>
        </div>
      ) : (
        <div className="upload-grid">
          {assets.map((item) => (
            <UploadCard
              key={item.id}
              asset={item}
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
  asset,
  refresh,
  setError,
}: {
  asset: Asset;
  refresh: () => Promise<void>;
  setError: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [password, setPassword] = useState("");
  const [expiresAt, setExpiresAt] = useState("");
  const [removePassword, setRemovePassword] = useState(false);
  const [clearExpiry, setClearExpiry] = useState(false);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState("");
  async function saveShare(event: FormEvent) {
    event.preventDefault();
    const problem = validateShare(password, expiresAt);
    if (problem) return setError(problem);
    setBusy(true);
    setError("");
    try {
      await api(`/api/assets/${asset.id}/share`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          enabled: true,
          ...(removePassword
            ? { password: null }
            : password
              ? { password }
              : {}),
          ...(clearExpiry
            ? { expiresAt: null }
            : expiresAt
              ? { expiresAt: new Date(expiresAt).toISOString() }
              : {}),
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
      {asset.deletedAt ? (
        <div className="download-preview">
          <Trash2 size={24} aria-hidden="true" />
          <p>In Trash</p>
        </div>
      ) : (
        <AssetPreview asset={asset} />
      )}
      <div className="upload-meta">
        <div>
          <strong title={asset.name}>{asset.name}</strong>
          <span>{formatBytes(asset.byteSize)}</span>
        </div>
        <time
          dateTime={asset.deletedAt || asset.createdAt}
          title={new Date(asset.deletedAt || asset.createdAt).toLocaleString()}
        >
          {asset.deletedAt && "Deleted "}
          {new Date(asset.deletedAt || asset.createdAt).toLocaleDateString()}
        </time>
      </div>
      {asset.share && (
        <div className="share-list">
          <div className="share-row">
            <div>
              <span className="visibility">
                Shared ·{" "}
                <time title={new Date(asset.share.sharedAt).toLocaleString()}>
                  {new Date(asset.share.sharedAt).toLocaleDateString()}
                </time>
              </span>
              {asset.share.passwordProtected && <span> · password</span>}
            </div>
            <div className="share-actions">
              <button
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(
                      new URL(`/s/${asset.share!.id}`, window.location.origin)
                        .href,
                    );
                    setCopied(asset.share!.id);
                  } catch {
                    setError(
                      "Copy failed. Open the link and copy it from your address bar.",
                    );
                  }
                }}
              >
                {copied === asset.share.id ? "Copied" : "Copy"}
              </button>
              <a href={`/s/${asset.share.id}`} target="_blank" rel="noreferrer">
                Open
              </a>
            </div>
          </div>
        </div>
      )}
      {asset.deletedAt ? (
        <div className="card-actions">
          <button
            className="secondary-button"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await api(`/api/assets/${asset.id}/restore`, {
                  method: "POST",
                });
                await refresh();
              } catch (e) {
                setError((e as Error).message);
              } finally {
                setBusy(false);
              }
            }}
          >
            <Undo2 size={18} aria-hidden="true" /> Restore
          </button>
        </div>
      ) : open ? (
        <form className="share-form" onSubmit={saveShare}>
          <p>
            {asset.share
              ? "Update link options"
              : "Anyone with the link can view this capture."}
          </p>
          <label>
            {asset.share?.passwordProtected
              ? "New password (leave blank to keep current)"
              : "Password (optional)"}
            <input
              type="password"
              minLength={8}
              maxLength={128}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="At least 8 characters"
            />
          </label>
          {asset.share?.passwordProtected && (
            <label className="check-label">
              <input
                type="checkbox"
                checked={removePassword}
                onChange={(e) => setRemovePassword(e.target.checked)}
              />{" "}
              Remove password
            </label>
          )}
          <label>
            {asset.share?.expiresAt
              ? "New expiry (leave blank to keep current)"
              : "Expiry (optional)"}
            <input
              type="datetime-local"
              value={expiresAt}
              onChange={(e) => setExpiresAt(e.target.value)}
            />
          </label>
          {asset.share?.expiresAt && (
            <label className="check-label">
              <input
                type="checkbox"
                checked={clearExpiry}
                onChange={(e) => setClearExpiry(e.target.checked)}
              />{" "}
              Clear expiry
            </label>
          )}
          <div className="button-row">
            <button className="primary-button" disabled={busy}>
              {busy ? "Saving…" : asset.share ? "Save options" : "Create link"}
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
          <div className="button-row">
            <button
              className="secondary-button"
              aria-label={asset.share ? "Edit sharing" : "Share"}
              title={asset.share ? "Edit sharing" : "Share"}
              disabled={busy}
              onClick={() => {
                setPassword("");
                setExpiresAt("");
                setRemovePassword(false);
                setClearExpiry(false);
                setOpen(true);
              }}
            >
              <Share2 size={18} aria-hidden="true" />
            </button>
            {asset.share && (
              <button
                className="secondary-button"
                aria-label="Stop sharing"
                title="Stop sharing"
                disabled={busy}
                onClick={async () => {
                  if (
                    !confirm(
                      "Stop sharing? This link will stop working immediately. Sharing again will create a new link.",
                    )
                  )
                    return;
                  setBusy(true);
                  try {
                    await api(`/api/assets/${asset.id}/share`, {
                      method: "PUT",
                      headers: { "Content-Type": "application/json" },
                      body: JSON.stringify({ enabled: false }),
                    });
                    await refresh();
                  } catch (e) {
                    setError((e as Error).message);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                <Link2Off size={18} aria-hidden="true" />
              </button>
            )}
          </div>
          <button
            className="danger-button"
            disabled={busy}
            onClick={async () => {
              if (
                !confirm(
                  "Move this capture to Trash? Its share link will stop working. You can restore the file later.",
                )
              )
                return;
              setBusy(true);
              try {
                await api(`/api/assets/${asset.id}`, { method: "DELETE" });
                await refresh();
              } catch (e) {
                setError((e as Error).message);
              } finally {
                setBusy(false);
              }
            }}
          >
            Move to Trash
          </button>
        </div>
      )}
    </article>
  );
}

function AssetPreview({ asset }: { asset: Asset }) {
  const url = `/media/assets/${encodeURIComponent(asset.id)}`;
  const kind = assetMediaKind(asset.contentType);
  if (kind === "image")
    return <img src={url} alt={asset.name} loading="lazy" />;
  if (kind === "video")
    return (
      <video src={url} controls preload="metadata" aria-label={asset.name} />
    );
  return (
    <a className="download-preview" href={url} download={asset.name}>
      Download {asset.name}
    </a>
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
