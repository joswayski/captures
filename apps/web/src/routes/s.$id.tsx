import { createFileRoute } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import {
  getRequestHeader,
  setResponseHeader,
} from "@tanstack/react-start/server";
import { Eye, EyeOff, LockKeyhole } from "lucide-react";
import { FormEvent, useState } from "react";
import { shareMediaKind, type SharePageData } from "../shareModel";

const loadShare = createServerFn({ method: "GET" })
  .validator((input: { id: string }) => input)
  .handler(async ({ data }) => {
    setResponseHeader("Cache-Control", "private, no-store");
    setResponseHeader("Referrer-Policy", "no-referrer");
    const { fetchShareMetadata } = await import("../server/shareMetadata");
    const result = await fetchShareMetadata(
      data.id,
      getRequestHeader("cookie"),
    );
    setResponseHeader("X-Robots-Tag", "noindex, nofollow");
    return result;
  });

export const Route = createFileRoute("/s/$id")({
  loader: ({ params }) => loadShare({ data: { id: params.id } }),
  head: () => {
    return {
      meta: [
        { title: "Shared capture — Captures" },
        { name: "description", content: "A capture shared with Captures." },
        { name: "robots", content: "noindex, nofollow" },
        { name: "referrer", content: "no-referrer" },
      ],
    };
  },
  component: SharePage,
  remountDeps: ({ params }) => params.id,
});

function SharePage() {
  const initial = Route.useLoaderData();
  const { id } = Route.useParams();
  const [data, setData] = useState<SharePageData>(initial);
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [showPassword, setShowPassword] = useState(false);
  async function unlock(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const response = await fetch(`/api/shares/${id}/unlock`, {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ password }),
      });
      if (!response.ok) {
        const body = (await response.json().catch(() => null)) as {
          error?: string;
        } | null;
        throw new Error(body?.error || "That password did not work.");
      }
      const metadata = await fetch(`/api/shares/${id}`, {
        credentials: "same-origin",
        cache: "no-store",
      });
      if (!metadata.ok) throw new Error("This share is no longer available.");
      setData({ kind: "ready", share: await metadata.json() });
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <main
      className={`viewer-shell ${data.kind === "ready" && data.share.passwordRequired && !data.share.mediaUrl ? "viewer-shell-protected" : ""}`}
    >
      {data.kind === "missing" ? (
        <ViewerMessage
          title="This capture isn’t available"
          copy="The link may be incorrect, expired, private, or revoked."
        />
      ) : data.kind === "unavailable" ? (
        <ViewerMessage
          title="Unable to load this capture"
          copy="The sharing service is temporarily unavailable. Try again later."
        />
      ) : data.share.passwordRequired && !data.share.mediaUrl ? (
        <section className="password-card">
          <div className="password-lock">
            <LockKeyhole size={22} aria-hidden="true" />
          </div>
          <a href="/" className="password-brand">
            Captures
          </a>
          <h1>Enter password</h1>
          <p>This capture is password protected.</p>
          <form className="password-form" onSubmit={unlock}>
            <label className="password-field">
              <span className="sr-only">Password</span>
              <input
                type={showPassword ? "text" : "password"}
                aria-label="Password"
                required
                autoFocus
                autoComplete="current-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
              <button
                type="button"
                aria-label={showPassword ? "Hide password" : "Show password"}
                onClick={() => setShowPassword((value) => !value)}
              >
                {showPassword ? <EyeOff size={19} /> : <Eye size={19} />}
              </button>
            </label>
            <button className="password-submit" disabled={busy}>
              {busy ? "Checking…" : "View capture"}
            </button>
            {error && (
              <p role="alert" className="password-error">
                {error}
              </p>
            )}
          </form>
        </section>
      ) : data.share.mediaUrl ? (
        <figure className="shared-media">
          <SharedMedia
            id={id}
            name={data.share.name}
            contentType={data.share.contentType}
            onError={() => setData({ kind: "missing" })}
          />
          <figcaption>
            {data.share.expiresAt
              ? `Available until ${new Date(data.share.expiresAt).toLocaleString()}`
              : "Shared with Captures"}
          </figcaption>
        </figure>
      ) : (
        <ViewerMessage
          title="This capture isn’t available"
          copy="Access to this share could not be confirmed."
        />
      )}
    </main>
  );
}

function SharedMedia({
  id,
  name,
  contentType,
  onError,
}: {
  id: string;
  name: string;
  contentType: string;
  onError: () => void;
}) {
  const url = `/api/shares/${encodeURIComponent(id)}/media`;
  const kind = shareMediaKind(contentType);
  if (kind === "image") return <img src={url} alt={name} onError={onError} />;
  if (kind === "video")
    return <video src={url} controls autoPlay={false} onError={onError} />;
  return (
    <div className="shared-download">
      <p>{name}</p>
      <a className="primary-button" href={url} download={name}>
        Download capture
      </a>
    </div>
  );
}
function ViewerMessage({ title, copy }: { title: string; copy: string }) {
  return (
    <section className="viewer-message">
      <h1>{title}</h1>
      <p>{copy}</p>
    </section>
  );
}
