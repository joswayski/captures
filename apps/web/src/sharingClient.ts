export interface AccountUser {
  id: string;
  email: string;
  quotaBytes: number;
  usedBytes: number;
  reservedBytes: number;
}

export interface LibraryAsset {
  id: string;
  share_id: string;
  share_url: string;
  title: string | null;
  kind: "screenshot" | "video" | "gif";
  status: "pending" | "ready" | "failed" | "deleting";
  access: "private" | "shared";
  original_mime_type: string;
  original_bytes: number;
  preview_mime_type: string | null;
  preview_bytes: number;
  width: number | null;
  height: number | null;
  duration_ms: number | null;
  upload_expires_at: string;
  share_expires_at: string | null;
  password_protected: boolean;
  created_at: string;
  updated_at: string;
}

export interface SharedAsset {
  share_id: string;
  title: string | null;
  kind: "screenshot" | "video" | "gif";
  mime_type: string;
  bytes: number;
  width: number | null;
  height: number | null;
  duration_ms: number | null;
  created_at: string;
  expires_at: string | null;
  media_url: string;
  preview_url: string | null;
}

export async function apiJson<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    credentials: "same-origin",
    ...init,
    headers: {
      ...(init?.body ? { "Content-Type": "application/json" } : {}),
      ...init?.headers,
    },
  });
  const body = response.status === 204 ? null : await response.json().catch(() => null);
  if (!response.ok) {
    const message = body && typeof body.error === "string" ? body.error : "Request failed";
    throw new ApiError(message, response.status, body);
  }
  return body as T;
}

export class ApiError extends Error {
  readonly status: number;
  readonly body: unknown;

  constructor(message: string, status: number, body: unknown) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.body = body;
  }
}

export function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 ** 2) return `${(value / 1024).toFixed(1)} KB`;
  if (value < 1024 ** 3) return `${(value / 1024 ** 2).toFixed(1)} MB`;
  return `${(value / 1024 ** 3).toFixed(2)} GB`;
}
