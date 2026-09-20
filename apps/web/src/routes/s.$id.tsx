import { createFileRoute } from "@tanstack/react-router";
import { createServerFn } from "@tanstack/react-start";
import {
  getRequestHeader,
  setResponseHeader,
} from "@tanstack/react-start/server";
import { FormEvent, useState } from "react";
import { mayIndex, type SharePageData } from "../shareModel";

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
    if (!mayIndex(result))
      setResponseHeader("X-Robots-Tag", "noindex, nofollow");
    return result;
  });

export const Route = createFileRoute("/s/$id")({
  loader: ({ params }) => loadShare({ data: { id: params.id } }),
  head: ({ loaderData }) => {
    const indexable = loaderData ? mayIndex(loaderData) : false;
    return {
      meta: [
        { title: "Shared image — Captures" },
        { name: "description", content: "An image shared with Captures." },
        {
          name: "robots",
          content: indexable ? "index, follow" : "noindex, nofollow",
        },
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
    <main className="viewer-shell">
      <header className="viewer-header">
        <a href="/" className="account-brand">
          Captures
        </a>
        <span>Shared image</span>
      </header>
      {data.kind === "missing" ? (
        <ViewerMessage
          title="This image isn’t available"
          copy="The link may be incorrect, expired, private, or revoked."
        />
      ) : data.kind === "unavailable" ? (
        <ViewerMessage
          title="Unable to load this image"
          copy="The sharing service is temporarily unavailable. Try again later."
        />
      ) : data.share.passwordRequired && !data.share.mediaUrl ? (
        <section className="viewer-message">
          <p className="eyebrow">Protected share</p>
          <h1>Password required</h1>
          <p>Enter the password from the person who shared this image.</p>
          <form className="form-stack" onSubmit={unlock}>
            <label>
              Password
              <input
                type="password"
                required
                autoFocus
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </label>
            <button className="primary-button" disabled={busy}>
              {busy ? "Unlocking…" : "View image"}
            </button>
          </form>
          {error && (
            <p role="alert" className="error-box">
              {error}
            </p>
          )}
        </section>
      ) : data.share.mediaUrl ? (
        <figure className="shared-image">
          <img
            src={`/api/shares/${encodeURIComponent(id)}/media`}
            alt="Shared capture"
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
          title="This image isn’t available"
          copy="Access to this share could not be confirmed."
        />
      )}
    </main>
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
