import { fileTypeFromBuffer } from "file-type";
import { GIB, type AssetKind, type CreateAssetInput } from "./types.ts";

const SHA256_BASE64 = /^[A-Za-z0-9+/]{43}=$/u;
const MIME_BY_KIND: Record<AssetKind, Set<string>> = {
  screenshot: new Set(["image/png", "image/jpeg", "image/webp"]),
  gif: new Set(["image/gif"]),
  video: new Set(["video/mp4", "video/webm"]),
};
const PREVIEW_MIMES = new Set(["image/png", "image/jpeg", "image/webp"]);

type ParseResult<T> = { ok: true; value: T } | { ok: false; error: string };

function object(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function positiveInteger(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0
    ? value
    : undefined;
}

function optionalPositiveInteger(value: unknown): number | undefined | null {
  if (value === undefined || value === null) return undefined;
  return positiveInteger(value) ?? null;
}

function parseUploadObject(
  value: unknown,
  allowedMimes: Set<string>,
  field: string,
): ParseResult<{ bytes: number; mimeType: string; sha256: string }> {
  const input = object(value);
  const bytes = positiveInteger(input?.bytes);
  const mimeType = typeof input?.mime_type === "string"
    ? input.mime_type.trim().toLowerCase()
    : "";
  const sha256 = typeof input?.sha256 === "string" ? input.sha256.trim() : "";
  if (!bytes || bytes > GIB) {
    return { ok: false, error: `${field}.bytes must be between 1 byte and 1 GiB` };
  }
  if (!allowedMimes.has(mimeType)) {
    return { ok: false, error: `${field}.mime_type is not supported` };
  }
  if (!SHA256_BASE64.test(sha256)) {
    return { ok: false, error: `${field}.sha256 must be a base64 SHA-256 digest` };
  }
  return { ok: true, value: { bytes, mimeType, sha256 } };
}

export function parseCreateAsset(value: unknown): ParseResult<CreateAssetInput> {
  const input = object(value);
  if (!input) return { ok: false, error: "request body must be a JSON object" };
  const kind = input.kind;
  if (kind !== "screenshot" && kind !== "video" && kind !== "gif") {
    return { ok: false, error: "kind must be screenshot, video, or gif" };
  }

  const original = parseUploadObject(input.original, MIME_BY_KIND[kind], "original");
  if (!original.ok) return original;
  const preview = input.preview === undefined
    ? undefined
    : parseUploadObject(input.preview, PREVIEW_MIMES, "preview");
  if (preview && !preview.ok) return preview;
  if (original.value.bytes + (preview?.value.bytes ?? 0) > GIB) {
    return { ok: false, error: "original and preview together must not exceed 1 GiB" };
  }

  const title = typeof input.title === "string" ? input.title.trim() : undefined;
  if (title && Array.from(title).length > 200) {
    return { ok: false, error: "title must be at most 200 characters" };
  }
  const width = optionalPositiveInteger(input.width);
  const height = optionalPositiveInteger(input.height);
  const durationMs = optionalPositiveInteger(input.duration_ms);
  if (width === null || height === null || durationMs === null) {
    return { ok: false, error: "dimensions and duration must be positive integers" };
  }

  return {
    ok: true,
    value: {
      title,
      kind,
      original: original.value,
      preview: preview?.value,
      width,
      height,
      durationMs,
    },
  };
}

export function expectedMimeMatches(
  kind: AssetKind,
  declaredMime: string,
  bytes: Uint8Array,
): Promise<boolean> {
  return fileTypeFromBuffer(bytes).then((type) => {
    if (!type) return false;
    if (type.mime !== declaredMime) return false;
    return MIME_BY_KIND[kind].has(type.mime);
  });
}

export function previewMimeMatches(
  declaredMime: string,
  bytes: Uint8Array,
): Promise<boolean> {
  return fileTypeFromBuffer(bytes).then(
    (type) => Boolean(type && type.mime === declaredMime && PREVIEW_MIMES.has(type.mime)),
  );
}

export function parseShareExpiry(
  input: Record<string, unknown> | undefined,
): Date | null | undefined {
  if (input?.access === "private") return null;
  if (input?.expires_at === null || input?.expires_at === undefined) {
    const seconds = input?.expires_in_seconds;
    if (seconds === null || seconds === undefined) return null;
    if (typeof seconds !== "number" || !Number.isSafeInteger(seconds) || seconds < 60) {
      return undefined;
    }
    const date = new Date(Date.now() + seconds * 1_000);
    return Number.isNaN(date.getTime()) ? undefined : date;
  }
  if (typeof input.expires_at !== "string") return undefined;
  const date = new Date(input.expires_at);
  return Number.isNaN(date.getTime()) || date.getTime() <= Date.now() ? undefined : date;
}
