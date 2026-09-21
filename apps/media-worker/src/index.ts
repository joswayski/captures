export interface Env {
  ASSETS: R2BucketLike;
  MEDIA_API_ORIGIN: string;
  MEDIA_WORKER_SECRET: string;
  MEDIA_AUTH_TIMEOUT_MS?: string;
}

interface R2ObjectLike {
  size: number;
  body?: ReadableStream;
}

interface R2BucketLike {
  head(key: string): Promise<R2ObjectLike | null>;
  get(key: string, options?: { range?: { offset?: number; length?: number; suffix?: number } }): Promise<R2ObjectLike | null>;
}

type MediaKind = "assets" | "shares";
type Authorization = { key: string; name: string; contentType: string; byteSize: number };

const ID = /^[A-Za-z0-9_-]{12}$/;
const SAFE_INLINE = new Set([
  "image/gif", "image/jpeg", "image/png", "image/webp",
  "video/mp4", "video/webm", "video/ogg",
]);

const securityHeaders = (): Headers => new Headers({
  "Cache-Control": "private, no-store",
  "CDN-Cache-Control": "no-store",
  "Cloudflare-CDN-Cache-Control": "no-store",
  "Content-Security-Policy": "default-src 'none'; sandbox",
  "Referrer-Policy": "no-referrer",
  "X-Content-Type-Options": "nosniff",
  "X-Robots-Tag": "noindex, nofollow, noarchive",
});

function error(status: number): Response {
  return new Response(null, { status, headers: securityHeaders() });
}

function origin(value: string): URL | null {
  try {
    const url = new URL(value);
    const local = url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    if (url.protocol !== "https:" && !local) return null;
    if (url.username || url.password || url.pathname !== "/" || url.search || url.hash) return null;
    return url;
  } catch {
    return null;
  }
}

async function authorize(request: Request, env: Env, kind: MediaKind, id: string): Promise<Authorization | Response> {
  const apiOrigin = origin(env.MEDIA_API_ORIGIN);
  if (!apiOrigin || !env.MEDIA_WORKER_SECRET) return error(503);
  const timeout = Number(env.MEDIA_AUTH_TIMEOUT_MS ?? "5000");
  if (!Number.isInteger(timeout) || timeout < 1 || timeout > 30_000) return error(503);

  const headers = new Headers({ "x-captures-media-key": env.MEDIA_WORKER_SECRET });
  for (const name of ["Cookie", "Authorization"]) {
    const value = request.headers.get(name);
    if (value !== null) headers.set(name, value);
  }

  let response: Response;
  try {
    response = await fetch(new URL(`/api/media/${kind}/${id}`, apiOrigin), {
      method: "GET",
      headers,
      // workerd supports manual/follow, not Fetch's redirect:"error" mode.
      // Any redirect is rejected below without forwarding viewer credentials.
      redirect: "manual",
      cache: "no-store",
      signal: AbortSignal.timeout(timeout),
    });
  } catch {
    return error(504);
  }
  if (!response.ok) {
    return error(response.status >= 400 && response.status < 500 ? response.status : 502);
  }

  let data: unknown;
  try {
    data = await response.json();
  } catch {
    return error(502);
  }
  if (!data || typeof data !== "object") return error(502);
  const value = data as Record<string, unknown>;
  // Shares have their own ID; only the API can resolve it to an asset key.
  if (typeof value.key !== "string" || !/^assets\/(?:[A-Za-z0-9_-]{12}\/)?[A-Za-z0-9_-]{12}$/.test(value.key) ||
      (kind === "assets" && !value.key.endsWith(`/${id}`)) ||
      typeof value.name !== "string" || value.name.length < 1 || value.name.length > 1024 ||
      typeof value.contentType !== "string" || value.contentType.length > 255 ||
      typeof value.byteSize !== "number" || !Number.isSafeInteger(value.byteSize) || value.byteSize < 0) return error(502);
  try {
    encodeURIComponent(value.name);
  } catch {
    return error(502);
  }
  return value as Authorization;
}

type ByteRange = { start: number; end: number };

function parseRange(header: string | null, size: number): ByteRange | null | "invalid" {
  if (header === null) return null;
  const match = /^bytes=(\d*)-(\d*)$/.exec(header);
  if (!match || (!match[1] && !match[2]) || size === 0) return "invalid";
  if (!match[1]) {
    const suffix = Number(match[2]);
    if (!Number.isSafeInteger(suffix) || suffix <= 0) return "invalid";
    return { start: Math.max(0, size - suffix), end: size - 1 };
  }
  const start = Number(match[1]);
  let end = match[2] ? Number(match[2]) : size - 1;
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start >= size || end < start) return "invalid";
  end = Math.min(end, size - 1);
  return { start, end };
}

function disposition(name: string, inline: boolean): string {
  const fallback = name.replace(/[^\x20-\x7e]/g, "_").replace(/["\\]/g, "_") || "download";
  const encoded = encodeURIComponent(name).replace(/[!'()*]/g, (character) => `%${character.charCodeAt(0).toString(16).toUpperCase()}`);
  return `${inline ? "inline" : "attachment"}; filename="${fallback}"; filename*=UTF-8''${encoded}`;
}

export async function handleRequest(request: Request, env: Env): Promise<Response> {
  if (request.method !== "GET" && request.method !== "HEAD") return error(405);
  const match = /^\/api\/files\/(assets|shares)\/([A-Za-z0-9_-]{12})$/.exec(new URL(request.url).pathname);
  if (!match || !ID.test(match[2])) return error(404);
  const kind = match[1] as MediaKind;
  const id = match[2];
  const authorization = await authorize(request, env, kind, id);
  if (authorization instanceof Response) return authorization;

  // Range applies to GET only; HEAD describes the complete representation.
  const range = parseRange(request.method === "GET" ? request.headers.get("Range") : null, authorization.byteSize);
  if (range === "invalid") {
    const response = error(416);
    response.headers.set("Content-Range", `bytes */${authorization.byteSize}`);
    return response;
  }

  let object: R2ObjectLike | null;
  try {
    object = request.method === "HEAD"
      ? await env.ASSETS.head(authorization.key)
      : await env.ASSETS.get(authorization.key, range ? { range: { offset: range.start, length: range.end - range.start + 1 } } : undefined);
  } catch {
    return error(502);
  }
  if (!object || object.size !== authorization.byteSize || (request.method === "GET" && !object.body)) return error(502);

  const inline = SAFE_INLINE.has(authorization.contentType.toLowerCase());
  const headers = securityHeaders();
  headers.set("Accept-Ranges", "bytes");
  headers.set("Content-Disposition", disposition(authorization.name, inline));
  headers.set("Content-Type", inline ? authorization.contentType : "application/octet-stream");
  headers.set("Content-Length", String(range ? range.end - range.start + 1 : authorization.byteSize));
  if (range) headers.set("Content-Range", `bytes ${range.start}-${range.end}/${authorization.byteSize}`);
  return new Response(request.method === "HEAD" ? null : object.body, { status: range ? 206 : 200, headers });
}

export default { fetch: handleRequest };
