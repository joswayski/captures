import { createFileRoute } from "@tanstack/react-router";
import { useCallback, useEffect, useState } from "react";
import {
  ApiError,
  apiJson,
  formatBytes,
  type AccountUser,
  type LibraryAsset,
} from "../sharingClient";

export const Route = createFileRoute("/library")({
  head: () => ({ meta: [{ title: "Library · Captures" }] }),
  component: LibraryPage,
});

function LibraryPage() {
  const [user, setUser] = useState<AccountUser | null>(null);
  const [assets, setAssets] = useState<LibraryAsset[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    try {
      const [account, library] = await Promise.all([
        apiJson<{ user: AccountUser }>("/api/me"),
        apiJson<{ assets: LibraryAsset[]; next_cursor: string | null }>("/api/assets"),
      ]);
      setUser(account.user);
      setAssets(library.assets);
      setNextCursor(library.next_cursor);
      setError("");
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) {
        window.location.assign("/login");
        return;
      }
      setError(error instanceof Error ? error.message : "Couldn’t load your library");
    } finally {
      setLoading(false);
    }
  }, []);

  async function loadMore() {
    if (!nextCursor || loadingMore) return;
    setLoadingMore(true);
    setError("");
    try {
      const page = await apiJson<{ assets: LibraryAsset[]; next_cursor: string | null }>(
        `/api/assets?cursor=${encodeURIComponent(nextCursor)}`,
      );
      setAssets((current) => [...current, ...page.assets]);
      setNextCursor(page.next_cursor);
    } catch (error) {
      setError(error instanceof Error ? error.message : "Couldn’t load more captures");
    } finally {
      setLoadingMore(false);
    }
  }

  useEffect(() => { void refresh(); }, [refresh]);

  async function signOut() {
    await apiJson("/api/auth/logout", { method: "POST" }).catch(() => {});
    window.location.assign("/");
  }

  const allocated = (user?.usedBytes ?? 0) + (user?.reservedBytes ?? 0);
  const quotaPercent = user ? Math.min(100, (allocated / user.quotaBytes) * 100) : 0;

  return (
    <main className="library-shell">
      <header className="library-header">
        <div>
          <a href="/" className="account-brand">Captures</a>
          <h1>Your library</h1>
          <p>{user?.email ?? "Private screenshots, GIFs, and videos"}</p>
        </div>
        <button type="button" className="library-signout" onClick={() => void signOut()}>Sign out</button>
      </header>

      {user && (
        <section className="quota-card" aria-label="Storage quota">
          <div><span>Storage</span><strong>{formatBytes(allocated)} of {formatBytes(user.quotaBytes)}</strong></div>
          <div className="quota-track"><span style={{ width: `${quotaPercent}%` }} /></div>
          {user.reservedBytes > 0 && <p>{formatBytes(user.reservedBytes)} reserved by uploads in progress</p>}
        </section>
      )}

      {error && <p className="account-error" role="alert">{error}</p>}
      {loading ? (
        <section className="library-empty"><h2>Loading your captures…</h2></section>
      ) : assets.length === 0 ? (
        <section className="library-empty">
          <h2>No remote captures yet</h2>
          <p>Use Share in the Captures desktop history. Local history stays on your device.</p>
        </section>
      ) : (
        <>
          <section className="library-grid" aria-label="Remote captures">
            {assets.map((asset) => (
              <LibraryCard
                key={asset.id}
                asset={asset}
                onChanged={(next) => setAssets((current) => current.map((item) => item.id === next.id ? next : item))}
                onDeleted={() => setAssets((current) => current.filter((item) => item.id !== asset.id))}
              />
            ))}
          </section>
          {nextCursor && (
            <button
              type="button"
              className="library-load-more"
              disabled={loadingMore}
              onClick={() => void loadMore()}
            >
              {loadingMore ? "Loading…" : "Load more"}
            </button>
          )}
        </>
      )}
    </main>
  );
}

function LibraryCard({
  asset,
  onChanged,
  onDeleted,
}: {
  asset: LibraryAsset;
  onChanged: (asset: LibraryAsset) => void;
  onDeleted: () => void;
}) {
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [busy, setBusy] = useState("");
  const [message, setMessage] = useState("");
  const [shareExpiry, setShareExpiry] = useState("never");
  const [customExpiry, setCustomExpiry] = useState("");

  useEffect(() => {
    let active = true;
    if (asset.status !== "ready") return;
    const variant = asset.preview_bytes > 0 ? "?variant=preview" : "";
    void apiJson<{ url: string }>(`/api/assets/${asset.id}/media${variant}`)
      .then(({ url }) => { if (active) setPreviewUrl(url); })
      .catch(() => {});
    return () => { active = false; };
  }, [asset.id, asset.preview_bytes, asset.status]);

  async function setAccess(
    access: "private" | "shared",
    expiry: { expiresInSeconds?: number; expiresAt?: string } = {},
  ) {
    setBusy("access");
    setMessage("");
    try {
      const result = await apiJson<{ asset: LibraryAsset }>(`/api/assets/${asset.id}/share`, {
        method: "PATCH",
        body: JSON.stringify({
          access,
          expires_in_seconds: expiry.expiresInSeconds ?? null,
          ...(expiry.expiresAt ? { expires_at: expiry.expiresAt } : {}),
        }),
      });
      onChanged(result.asset);
      if (access === "shared") {
        try {
          await navigator.clipboard.writeText(result.asset.share_url);
          setMessage("Shared link copied");
        } catch {
          setMessage(`Shared · ${result.asset.share_url}`);
        }
      } else {
        setMessage("Link disabled");
      }
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Couldn’t update sharing");
    } finally {
      setBusy("");
    }
  }

  async function shareWithSelectedExpiry() {
    if (shareExpiry === "custom") {
      const expiresAt = new Date(customExpiry);
      if (!customExpiry || Number.isNaN(expiresAt.getTime()) || expiresAt.getTime() <= Date.now()) {
        setMessage("Choose a future custom expiry");
        return;
      }
      await setAccess("shared", { expiresAt: expiresAt.toISOString() });
      return;
    }
    const days = shareExpiry === "never" ? null : Number(shareExpiry);
    await setAccess("shared", {
      expiresInSeconds: days === null ? undefined : days * 24 * 60 * 60,
    });
  }

  async function rotate() {
    setBusy("rotate");
    try {
      const result = await apiJson<{ asset: LibraryAsset }>(`/api/assets/${asset.id}/share`, { method: "POST" });
      onChanged(result.asset);
      try {
        await navigator.clipboard.writeText(result.asset.share_url);
        setMessage("New link copied; the old link no longer works");
      } catch {
        setMessage(`Link rotated; the old link no longer works · ${result.asset.share_url}`);
      }
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Couldn’t rotate link");
    } finally {
      setBusy("");
    }
  }

  async function remove() {
    if (!window.confirm("Delete this remote capture? Your local history is unaffected.")) return;
    setBusy("delete");
    try {
      await apiJson(`/api/assets/${asset.id}`, { method: "DELETE" });
      onDeleted();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "Couldn’t delete capture");
      setBusy("");
    }
  }

  return (
    <article className="library-card">
      <div className="library-preview">
        {previewUrl ? (
          asset.kind === "video" && asset.preview_bytes === 0
            ? <video src={previewUrl} muted preload="metadata" />
            : <img src={previewUrl} alt="" />
        ) : <span>{asset.status === "pending" ? "Upload pending" : asset.kind}</span>}
      </div>
      <div className="library-card-body">
        <div className="library-card-meta">
          <strong>{asset.title || `${asset.kind[0].toUpperCase()}${asset.kind.slice(1)}`}</strong>
          <span>{formatBytes(asset.original_bytes)} · {new Date(asset.created_at).toLocaleDateString()}</span>
        </div>
        {asset.status === "ready" && (
          <div className="library-actions">
            {asset.access === "private" ? (
              <ShareExpiryControls
                busy={Boolean(busy)}
                value={shareExpiry}
                customValue={customExpiry}
                action="Share"
                onValue={setShareExpiry}
                onCustomValue={setCustomExpiry}
                onSubmit={() => void shareWithSelectedExpiry()}
              />
            ) : (
              <>
                <button type="button" disabled={Boolean(busy)} onClick={() => void navigator.clipboard.writeText(asset.share_url).then(() => setMessage("Link copied"))}>Copy link</button>
                <button type="button" disabled={Boolean(busy)} onClick={() => void setAccess("private")}>Make private</button>
                <button type="button" disabled={Boolean(busy)} onClick={() => void rotate()}>Rotate link</button>
                <ShareExpiryControls
                  busy={Boolean(busy)}
                  value={shareExpiry}
                  customValue={customExpiry}
                  action="Update expiry"
                  onValue={setShareExpiry}
                  onCustomValue={setCustomExpiry}
                  onSubmit={() => void shareWithSelectedExpiry()}
                />
              </>
            )}
            <button type="button" className="danger" disabled={Boolean(busy)} onClick={() => void remove()}>Delete remote</button>
          </div>
        )}
        {asset.status === "pending" && (
          <div className="library-actions">
            <button type="button" className="danger" disabled={Boolean(busy)} onClick={() => void remove()}>
              {busy === "delete" ? "Deleting…" : "Cancel upload and delete remote"}
            </button>
          </div>
        )}
        {asset.access === "shared" && (
          <p className="library-share-state">
            Shared{asset.share_expires_at ? ` until ${new Date(asset.share_expires_at).toLocaleString()}` : " until you disable it"}
          </p>
        )}
        {message && <p className="library-message" role="status">{message}</p>}
      </div>
    </article>
  );
}

function ShareExpiryControls({
  busy,
  value,
  customValue,
  action,
  onValue,
  onCustomValue,
  onSubmit,
}: {
  busy: boolean;
  value: string;
  customValue: string;
  action: string;
  onValue: (value: string) => void;
  onCustomValue: (value: string) => void;
  onSubmit: () => void;
}) {
  return (
    <div className="library-share-options">
      <select value={value} disabled={busy} aria-label="Shared link expiry" onChange={(event) => onValue(event.target.value)}>
        <option value="never">No expiry</option>
        <option value="1">1 day</option>
        <option value="7">7 days</option>
        <option value="30">30 days</option>
        <option value="custom">Custom</option>
      </select>
      {value === "custom" && (
        <input
          type="datetime-local"
          value={customValue}
          disabled={busy}
          aria-label="Custom shared link expiry"
          onChange={(event) => onCustomValue(event.target.value)}
        />
      )}
      <button type="button" disabled={busy} onClick={onSubmit}>{action}</button>
    </div>
  );
}
