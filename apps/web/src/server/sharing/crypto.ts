import {
  createHash,
  createHmac,
  randomBytes,
  randomInt,
  timingSafeEqual,
} from "node:crypto";
import { nanoid } from "nanoid";

export function newId(): string {
  return nanoid();
}

export function newOpaqueToken(): string {
  return randomBytes(32).toString("base64url");
}

export function hashToken(token: string): Buffer {
  return createHash("sha256").update(token, "utf8").digest();
}

export function generateLoginCode(): string {
  return randomInt(0, 1_000_000).toString().padStart(6, "0");
}

export function loginCodeHmac(key: Buffer, email: string, code: string): Buffer {
  return createHmac("sha256", key)
    .update("captures-login-code\0", "utf8")
    .update(email, "utf8")
    .update("\0", "utf8")
    .update(code, "utf8")
    .digest();
}

export function clientIpHmac(key: Buffer, ip: string): Buffer {
  return createHmac("sha256", key)
    .update("captures-client-ip\0", "utf8")
    .update(ip, "utf8")
    .digest();
}

export function safeEqual(left: Uint8Array, right: Uint8Array): boolean {
  return left.byteLength === right.byteLength && timingSafeEqual(left, right);
}

export function parseCookie(request: Request, name: string): string | undefined {
  const cookie = request.headers.get("Cookie");
  if (!cookie) return undefined;
  for (const item of cookie.split(";")) {
    const separator = item.indexOf("=");
    if (separator < 0) continue;
    if (item.slice(0, separator).trim() !== name) continue;
    const value = item.slice(separator + 1).trim();
    try {
      return decodeURIComponent(value);
    } catch {
      return undefined;
    }
  }
  return undefined;
}

export function bearerToken(request: Request): string | undefined {
  const authorization = request.headers.get("Authorization");
  if (!authorization) return undefined;
  const match = authorization.match(/^Bearer\s+([^\s]+)$/iu);
  return match?.[1];
}

export function sessionToken(request: Request): string | undefined {
  return bearerToken(request) ?? parseCookie(request, "captures_session");
}

export function webSessionCookie(token: string, maxAgeSeconds: number): string {
  return [
    `captures_session=${encodeURIComponent(token)}`,
    "Path=/",
    "HttpOnly",
    "Secure",
    "SameSite=Lax",
    `Max-Age=${maxAgeSeconds}`,
  ].join("; ");
}

export function clearWebSessionCookie(): string {
  return "captures_session=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0";
}
