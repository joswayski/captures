import { createFileRoute } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { ApiError, apiJson, formatBytes, type SharedAsset } from "../sharingClient";

export const Route = createFileRoute("/$shareId")({
  head: () => ({ meta: [{ title: "Shared capture · Captures" }] }),
  component: SharedCapturePage,
});

function SharedCapturePage() {
  const { shareId } = Route.useParams();
  const [asset, setAsset] = useState<SharedAsset | null>(null);
  const [state, setState] = useState<"loading" | "ready" | "missing" | "password" | "error">("loading");

  useEffect(() => {
    let active = true;
    void apiJson<{ asset: SharedAsset }>(`/api/shares/${shareId}`)
      .then(({ asset }) => {
        if (!active) return;
        setAsset(asset);
        setState("ready");
      })
      .catch((error) => {
        if (!active) return;
        if (error instanceof ApiError && error.status === 401) setState("password");
        else if (error instanceof ApiError && error.status === 404) setState("missing");
        else setState("error");
      });
    return () => { active = false; };
  }, [shareId]);

  if (state !== "ready" || !asset) {
    const copy = state === "loading"
      ? ["Loading capture…", ""]
      : state === "password"
        ? ["Password required", "Password-protected shares are coming soon."]
        : state === "missing"
          ? ["This link is unavailable", "It may be private, expired, rotated, or deleted."]
          : ["Couldn’t load this capture", "Please try again in a moment."];
    return (
      <main className="shared-shell shared-state">
        <a href="/" className="account-brand">Captures</a>
        <h1>{copy[0]}</h1>
        {copy[1] && <p>{copy[1]}</p>}
      </main>
    );
  }

  return (
    <main className="shared-shell">
      <header className="shared-header">
        <a href="/" className="account-brand">Captures</a>
        <div>
          <h1>{asset.title || "Shared capture"}</h1>
          <p>{formatBytes(asset.bytes)}{asset.width && asset.height ? ` · ${asset.width} × ${asset.height}` : ""}</p>
        </div>
      </header>
      <section className="shared-media">
        {asset.kind === "video" ? (
          <video src={asset.media_url} poster={asset.preview_url ?? undefined} controls autoPlay={false} />
        ) : (
          <img src={asset.media_url} alt={asset.title || "Shared capture"} />
        )}
      </section>
      {asset.expires_at && <p className="shared-expiry">This link expires {new Date(asset.expires_at).toLocaleString()}.</p>}
    </main>
  );
}
