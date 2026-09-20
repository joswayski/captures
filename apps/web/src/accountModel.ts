export type AssetShare = {
  id: string;
  passwordProtected: boolean;
  expiresAt: string | null;
  sharedAt: string;
};

export type Asset = {
  id: string;
  name: string;
  contentType: string;
  byteSize: number;
  createdAt: string;
  share: AssetShare | null;
};

type UploadSession = { id: string; partSize: number; partCount: number };

export function validateUpload(file: Pick<File, "size">): string | null {
  return file.size === 0 ? "That file is empty." : null;
}

export function validateShare(
  password: string,
  expiresAt: string,
): string | null {
  if (password && (password.length < 8 || password.length > 128)) {
    return "Passwords must be between 8 and 128 characters.";
  }
  if (
    expiresAt &&
    (!Number.isFinite(new Date(expiresAt).getTime()) ||
      new Date(expiresAt).getTime() <= Date.now())
  ) {
    return "Expiry must be in the future.";
  }
  return null;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024 * 1024)
    return `${Math.max(1, Math.round(bytes / 1024))} KiB`;
  if (bytes < 1024 * 1024 * 1024)
    return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
}

export function assetMediaKind(
  contentType: string,
): "image" | "video" | "download" {
  if (
    ["image/gif", "image/jpeg", "image/png", "image/webp"].includes(contentType)
  )
    return "image";
  if (["video/mp4", "video/webm", "video/ogg"].includes(contentType))
    return "video";
  return "download";
}

async function json<T>(
  fetcher: typeof fetch,
  url: string,
  init: RequestInit,
): Promise<T> {
  const response = await fetcher(url, { credentials: "same-origin", ...init });
  if (!response.ok) {
    const body = (await response.json().catch(() => null)) as {
      error?: string;
    } | null;
    throw new Error(body?.error || `Request failed (${response.status})`);
  }
  return response.json() as Promise<T>;
}

export async function uploadAsset(
  file: File,
  options: {
    signal: AbortSignal;
    onProgress?: (completed: number, total: number) => void;
    fetcher?: typeof fetch;
    retries?: number;
  },
): Promise<Asset> {
  const fetcher = options.fetcher ?? fetch;
  const session = await json<UploadSession>(fetcher, "/api/assets", {
    method: "POST",
    signal: options.signal,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      name: file.name,
      contentType: file.type || "application/octet-stream",
      byteSize: file.size,
    }),
  });
  if (session.partSize <= 0 || session.partCount <= 0)
    throw new Error("The upload could not be prepared.");

  try {
    const parts: Array<{ partNumber: number; etag: string }> = [];
    for (let partNumber = 1; partNumber <= session.partCount; partNumber += 1) {
      const { url, headers } = await json<{
        url: string;
        headers: Record<string, string>;
      }>(fetcher, `/api/assets/${encodeURIComponent(session.id)}/parts`, {
        method: "POST",
        signal: options.signal,
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ partNumber }),
      });
      const chunk = file.slice(
        (partNumber - 1) * session.partSize,
        Math.min(file.size, partNumber * session.partSize),
      );
      let response: Response | undefined;
      let lastError: unknown;
      for (let attempt = 0; attempt <= (options.retries ?? 2); attempt += 1) {
        try {
          response = await fetcher(url, {
            method: "PUT",
            headers,
            body: chunk,
            credentials: "omit",
            signal: options.signal,
          });
          if (response.ok) break;
          lastError = new Error(`Storage upload failed (${response.status})`);
        } catch (error) {
          if (options.signal.aborted) throw error;
          lastError = error;
        }
      }
      if (!response?.ok)
        throw lastError instanceof Error
          ? lastError
          : new Error("Storage upload failed.");
      const etag = response.headers.get("ETag");
      if (!etag)
        throw new Error(
          "Storage did not confirm the uploaded part. Check R2 CORS exposes ETag.",
        );
      parts.push({ partNumber, etag });
      options.onProgress?.(partNumber, session.partCount);
    }

    return await json<Asset>(
      fetcher,
      `/api/assets/${encodeURIComponent(session.id)}/complete`,
      {
        method: "POST",
        signal: options.signal,
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ parts }),
      },
    );
  } catch (error) {
    // Cancellation uses a fresh signal: the upload signal is already aborted.
    // A failed cleanup remains covered by the pending-upload lifecycle.
    await fetcher(`/api/assets/${encodeURIComponent(session.id)}`, {
      method: "DELETE",
      credentials: "same-origin",
      signal: AbortSignal.timeout(10_000),
    }).catch(() => undefined);
    throw error;
  }
}
