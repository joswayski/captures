import { createHash, createHmac } from "node:crypto";
import { clientKeyFromRequest } from "../env.ts";
import { verifyLoginCodeAttempt } from "./auth.ts";
import { clientIpAllowed, normalizeEmail, validateSharingConfig } from "./config.ts";
import {
  clearWebSessionCookie,
  clientIpHmac,
  generateLoginCode,
  hashToken,
  loginCodeHmac,
  newId,
  newOpaqueToken,
  parseCookie,
  safeEqual,
  sessionToken,
  webSessionCookie,
} from "./crypto.ts";
import { SharingDatabase } from "./database.ts";
import { createSesSmtpMailer } from "./mailer.ts";
import {
  confirmSnsSubscription,
  extractSuppressionEvents,
  verifySnsEnvelope,
} from "./sns.ts";
import { S3CompatibleStorage } from "./storage.ts";
import {
  MULTIPART_PART_BYTES,
  SINGLE_UPLOAD_MAX_BYTES,
  UPLOAD_TTL_MS,
  type AssetRecord,
  type ClientKind,
  type Mailer,
  type MultipartPart,
  type ObjectStorage,
  type SessionUser,
  type SharingConfig,
  type UploadTarget,
} from "./types.ts";
import {
  expectedMimeMatches,
  parseCreateAsset,
  parseShareExpiry,
  previewMimeMatches,
} from "./validation.ts";

const JSON_BODY_LIMIT = 64 * 1024;
const LOGIN_CODE_TTL_MS = 10 * 60 * 1_000;
const WEB_SESSION_TTL_SECONDS = 30 * 24 * 60 * 60;
const DESKTOP_ACCESS_TTL_MS = 15 * 60 * 1_000;
const DESKTOP_REFRESH_TTL_MS = 30 * 24 * 60 * 60 * 1_000;
const NANOID_PATH = "[A-Za-z0-9_-]{21}";

export interface SharingApi {
  handle(request: Request): Promise<Response | null>;
  close(): Promise<void>;
}

export async function createSharingApi(config: SharingConfig): Promise<SharingApi> {
  const missing = validateSharingConfig(config);
  if (missing.length > 0) {
    throw new Error(`sharing configuration is incomplete: ${missing.join(", ")}`);
  }
  const database = new SharingDatabase(
    config.databaseUrl!,
    config.migrationDatabaseUrl!,
  );
  await database.start();
  const storage = new S3CompatibleStorage({
    endpoint: config.storage.endpoint!,
    region: config.storage.region,
    bucket: config.storage.bucket,
    accessKeyId: config.storage.accessKeyId!,
    secretAccessKey: config.storage.secretAccessKey!,
  });
  const api = new SharingApiImpl(config, database, storage, createSesSmtpMailer(config.mail));
  api.startCleanup();
  return api;
}

export function createSharingApiForTest(options: {
  config: SharingConfig;
  database: SharingDatabase;
  storage: ObjectStorage;
  mailer: Mailer;
  fetcher?: typeof fetch;
}): SharingApi {
  return new SharingApiImpl(
    options.config,
    options.database,
    options.storage,
    options.mailer,
    options.fetcher,
  );
}

class SharingApiImpl implements SharingApi {
  readonly #config: SharingConfig;
  readonly #database: SharingDatabase;
  readonly #storage: ObjectStorage;
  readonly #mailer: Mailer;
  readonly #fetch: typeof fetch;
  #cleanupTimer?: NodeJS.Timeout;

  constructor(
    config: SharingConfig,
    database: SharingDatabase,
    storage: ObjectStorage,
    mailer: Mailer,
    fetcher: typeof fetch = fetch,
  ) {
    this.#config = config;
    this.#database = database;
    this.#storage = storage;
    this.#mailer = mailer;
    this.#fetch = fetcher;
  }

  startCleanup(): void {
    void this.#cleanupExpiredUploads();
    this.#cleanupTimer = setInterval(() => {
      void this.#cleanupExpiredUploads();
    }, 60 * 60 * 1_000);
    this.#cleanupTimer.unref();
  }

  async close(): Promise<void> {
    if (this.#cleanupTimer) clearInterval(this.#cleanupTimer);
    await this.#database.close();
  }

  async handle(request: Request): Promise<Response | null> {
    const url = new URL(request.url);
    const { pathname } = url;

    if (pathname === "/api/auth/providers") {
      return request.method === "GET"
        ? json({ google: Boolean(this.#config.auth.googleClientId && this.#config.auth.googleClientSecret) })
        : methodNotAllowed(["GET"]);
    }
    if (pathname === "/api/auth/email/start") {
      return request.method === "POST"
        ? this.#startEmailLogin(request)
        : methodNotAllowed(["POST"]);
    }
    if (pathname === "/api/auth/email/verify") {
      return request.method === "POST"
        ? this.#verifyEmailLogin(request)
        : methodNotAllowed(["POST"]);
    }
    if (pathname === "/api/auth/google/start") {
      return request.method === "GET"
        ? this.#startGoogleLogin(request)
        : methodNotAllowed(["GET"]);
    }
    if (pathname === "/api/auth/google/callback") {
      return request.method === "GET"
        ? this.#completeGoogleLogin(request)
        : methodNotAllowed(["GET"]);
    }
    if (pathname === "/api/auth/refresh") {
      return request.method === "POST"
        ? this.#refreshDesktopSession(request)
        : methodNotAllowed(["POST"]);
    }
    if (pathname === "/api/auth/logout") {
      return request.method === "POST" ? this.#logout(request) : methodNotAllowed(["POST"]);
    }
    if (pathname === "/api/me") {
      return request.method === "GET" ? this.#me(request) : methodNotAllowed(["GET"]);
    }
    if (pathname === "/api/email/events") {
      return request.method === "POST"
        ? this.#receiveEmailEvent(request)
        : methodNotAllowed(["POST"]);
    }
    if (pathname === "/api/assets") {
      if (request.method === "GET") return this.#listAssets(request);
      if (request.method === "POST") return this.#createAsset(request);
      return methodNotAllowed(["GET", "POST"]);
    }

    let match = pathname.match(new RegExp(`^/api/assets/(${NANOID_PATH})$`, "u"));
    if (match) {
      if (request.method === "GET") return this.#getAsset(request, match[1]);
      if (request.method === "DELETE") return this.#deleteAsset(request, match[1]);
      return methodNotAllowed(["GET", "DELETE"]);
    }
    match = pathname.match(new RegExp(`^/api/assets/(${NANOID_PATH})/complete$`, "u"));
    if (match) {
      return request.method === "POST"
        ? this.#completeAsset(request, match[1])
        : methodNotAllowed(["POST"]);
    }
    match = pathname.match(new RegExp(`^/api/assets/(${NANOID_PATH})/parts$`, "u"));
    if (match) {
      return request.method === "POST"
        ? this.#refreshParts(request, match[1])
        : methodNotAllowed(["POST"]);
    }
    match = pathname.match(new RegExp(`^/api/assets/(${NANOID_PATH})/share$`, "u"));
    if (match) {
      if (request.method === "PATCH") return this.#updateShare(request, match[1]);
      if (request.method === "POST") return this.#rotateShare(request, match[1]);
      return methodNotAllowed(["PATCH", "POST"]);
    }
    match = pathname.match(new RegExp(`^/api/assets/(${NANOID_PATH})/media$`, "u"));
    if (match) {
      return request.method === "GET"
        ? this.#ownerMedia(request, match[1])
        : methodNotAllowed(["GET"]);
    }
    match = pathname.match(new RegExp(`^/api/shares/(${NANOID_PATH})$`, "u"));
    if (match) {
      return request.method === "GET"
        ? this.#sharedAsset(request, match[1])
        : methodNotAllowed(["GET"]);
    }

    return pathname.startsWith("/api/auth/") ||
      pathname.startsWith("/api/assets") ||
      pathname.startsWith("/api/shares/") ||
      pathname === "/api/me" ||
      pathname === "/api/email/events"
      ? json({ error: "not found" }, 404)
      : null;
  }

  async #startEmailLogin(request: Request): Promise<Response> {
    const parsed = await readJson(request);
    if (!parsed.ok) return json({ error: parsed.error }, parsed.status);
    const input = record(parsed.value);
    const email = normalizeEmail(string(input?.email) ?? "");
    const clientKind = parseClientKind(input?.client);
    if (!validEmail(email) || !clientKind) {
      return json({ error: "email and a valid client are required" }, 400);
    }
    const ip = clientKeyFromRequest(request);
    const allowed = await this.#interactiveAuthAllowed(email, ip, input);
    if (!allowed) return genericLoginStart();

    const key = this.#config.auth.codeHmacKey!;
    const ipHmac = clientIpHmac(key, ip);
    if (await this.#database.isEmailSuppressed(email)) return genericLoginStart();
    const id = newId();
    const code = generateLoginCode();
    const rate = await this.#database.reserveLoginCode({
      id,
      email,
      clientKind,
      codeHmac: loginCodeHmac(key, email, code),
      ipHmac,
      expiresAt: new Date(Date.now() + LOGIN_CODE_TTL_MS),
    });
    if (!rate.allowed) return genericLoginStart(rate.retryAfterSeconds);
    try {
      const messageId = await this.#mailer.sendLoginCode(email, code);
      await this.#database.attachSesMessageId(id, messageId);
    } catch (error) {
      await this.#database.invalidateLoginCode(id).catch(() => {});
      console.error("failed to send Captures login code", error);
      return json({ error: "email service is temporarily unavailable" }, 503);
    }
    return genericLoginStart();
  }

  async #verifyEmailLogin(request: Request): Promise<Response> {
    const parsed = await readJson(request);
    if (!parsed.ok) return json({ error: parsed.error }, parsed.status);
    const input = record(parsed.value);
    const email = normalizeEmail(string(input?.email) ?? "");
    const code = string(input?.code)?.trim() ?? "";
    const clientKind = parseClientKind(input?.client);
    if (!validEmail(email) || !/^\d{6}$/u.test(code) || !clientKind) {
      return json({ error: "the sign-in code is invalid or expired" }, 400);
    }
    if (!(await this.#interactiveAuthAllowed(email, clientKeyFromRequest(request), input))) {
      return json({ error: "the sign-in code is invalid or expired" }, 400);
    }

    try {
      const issued = await this.#database.withTransaction((client) =>
        verifyLoginCodeAttempt(client, {
          findLoginCode: (transaction) =>
            this.#database.latestLoginCode(transaction, email, clientKind),
          expectedCodeHmac: loginCodeHmac(this.#config.auth.codeHmacKey!, email, code),
          recordFailedAttempt: (transaction, id) =>
            this.#database.recordFailedLoginAttempt(transaction, id),
          consumeLoginCode: (transaction, id) =>
            this.#database.consumeLoginCode(transaction, id),
          onSuccess: async (transaction) => {
            const user = await this.#database.upsertUser(transaction, newId(), email);
            return this.#issueSession(transaction, user, clientKind);
          },
        }),
      );
      if (!issued) {
        return json({ error: "the sign-in code is invalid or expired" }, 400);
      }
      return sessionResponse(issued, clientKind);
    } catch (error) {
      if (error instanceof Error && error.message === "account_suspended") {
        return json({ error: "this account is unavailable" }, 403);
      }
      return json({ error: "the sign-in code is invalid or expired" }, 400);
    }
  }

  async #startGoogleLogin(request: Request): Promise<Response> {
    const clientId = this.#config.auth.googleClientId;
    if (!clientId || !this.#config.auth.googleClientSecret) {
      return json({ error: "Google sign-in is not configured" }, 503);
    }
    const ip = clientKeyFromRequest(request);
    if (!clientIpAllowed(ip, this.#config.auth.allowedCidrs)) {
      return json({ error: "not found" }, 404);
    }
    const state = newOpaqueToken();
    const verifier = `${newOpaqueToken()}${newOpaqueToken()}`;
    const challenge = createHash("sha256").update(verifier).digest("base64url");
    const oauthCookie = signOauthState(
      this.#config.auth.codeHmacKey!,
      { state, verifier, expiresAt: Date.now() + 10 * 60 * 1_000 },
    );
    const callback = `${this.#config.publicOrigin}/api/auth/google/callback`;
    const url = new URL("https://accounts.google.com/o/oauth2/v2/auth");
    url.search = new URLSearchParams({
      client_id: clientId,
      redirect_uri: callback,
      response_type: "code",
      scope: "openid email",
      state,
      code_challenge: challenge,
      code_challenge_method: "S256",
      prompt: "select_account",
    }).toString();
    return redirect(url.href, {
      "Set-Cookie": `captures_oauth=${encodeURIComponent(oauthCookie)}; Path=/api/auth/google/callback; HttpOnly; Secure; SameSite=Lax; Max-Age=600`,
    });
  }

  async #completeGoogleLogin(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const state = url.searchParams.get("state") ?? "";
    const code = url.searchParams.get("code") ?? "";
    const oauth = verifyOauthState(
      this.#config.auth.codeHmacKey!,
      parseCookie(request, "captures_oauth") ?? "",
    );
    if (!oauth || oauth.state !== state || oauth.expiresAt <= Date.now() || !code) {
      return redirect(`${this.#config.publicOrigin}/login?error=google`);
    }
    try {
      const callback = `${this.#config.publicOrigin}/api/auth/google/callback`;
      const tokenResponse = await this.#fetch("https://oauth2.googleapis.com/token", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          code,
          client_id: this.#config.auth.googleClientId!,
          client_secret: this.#config.auth.googleClientSecret!,
          redirect_uri: callback,
          grant_type: "authorization_code",
          code_verifier: oauth.verifier,
        }),
        signal: AbortSignal.timeout(10_000),
      });
      if (!tokenResponse.ok) throw new Error(`Google token exchange failed: ${tokenResponse.status}`);
      const tokens = record(await tokenResponse.json());
      const idToken = string(tokens?.id_token);
      if (!idToken) throw new Error("Google did not return an ID token");
      const identityResponse = await this.#fetch(
        `https://oauth2.googleapis.com/tokeninfo?id_token=${encodeURIComponent(idToken)}`,
        { signal: AbortSignal.timeout(10_000) },
      );
      if (!identityResponse.ok) throw new Error("Google ID token validation failed");
      const identity = record(await identityResponse.json());
      const email = normalizeEmail(string(identity?.email) ?? "");
      const subject = string(identity?.sub) ?? "";
      if (
        identity?.aud !== this.#config.auth.googleClientId ||
        identity?.email_verified !== "true" ||
        !validEmail(email) ||
        !subject ||
        !this.#config.auth.allowedEmails.has(email)
      ) {
        throw new Error("Google account is not allowed");
      }
      const issued = await this.#database.withTransaction(async (client) => {
        const user = await this.#database.upsertGoogleUser(client, newId(), email, subject);
        return this.#issueSession(client, user, "web");
      });
      return redirect(`${this.#config.publicOrigin}/library`, {
        "Set-Cookie": webSessionCookie(issued.accessToken, WEB_SESSION_TTL_SECONDS),
      });
    } catch (error) {
      console.error("Google sign-in failed", error);
      return redirect(`${this.#config.publicOrigin}/login?error=google`);
    }
  }

  async #refreshDesktopSession(request: Request): Promise<Response> {
    const parsed = await readJson(request);
    if (!parsed.ok) return json({ error: parsed.error }, parsed.status);
    const refreshToken = string(record(parsed.value)?.refresh_token);
    if (!refreshToken) return json({ error: "refresh token is required" }, 400);
    const accessToken = newOpaqueToken();
    const user = await this.#database.rotateDesktopSession(
      hashToken(refreshToken),
      hashToken(accessToken),
      new Date(Date.now() + DESKTOP_ACCESS_TTL_MS),
    );
    return user
      ? json({ access_token: accessToken, expires_in: DESKTOP_ACCESS_TTL_MS / 1_000, user })
      : json({ error: "session expired" }, 401);
  }

  async #logout(request: Request): Promise<Response> {
    const token = sessionToken(request);
    if (token) await this.#database.revokeSession(hashToken(token));
    const response = json({ ok: true });
    response.headers.set("Set-Cookie", clearWebSessionCookie());
    return response;
  }

  async #me(request: Request): Promise<Response> {
    const user = await this.#authenticatedUser(request);
    return user ? json({ user }) : json({ error: "authentication required" }, 401);
  }

  async #listAssets(request: Request): Promise<Response> {
    const user = await this.#authenticatedUser(request);
    if (!user) return json({ error: "authentication required" }, 401);
    const cursor = new URL(request.url).searchParams.get("cursor");
    if (cursor && !new RegExp(`^${NANOID_PATH}$`, "u").test(cursor)) {
      return json({ error: "cursor is invalid" }, 400);
    }
    const page = await this.#database.listAssets(user.id, cursor, 101);
    const hasMore = page.length > 100;
    const assets = hasMore ? page.slice(0, 100) : page;
    return json({
      assets: assets.map((asset) => ownerAssetJson(asset, this.#config.publicOrigin)),
      next_cursor: hasMore ? assets.at(-1)?.id ?? null : null,
    });
  }

  async #getAsset(request: Request, assetId: string): Promise<Response> {
    const user = await this.#authenticatedUser(request);
    if (!user) return json({ error: "authentication required" }, 401);
    const asset = await this.#database.getOwnerAsset(user.id, assetId);
    return asset
      ? json({ asset: ownerAssetJson(asset, this.#config.publicOrigin) })
      : json({ error: "asset not found" }, 404);
  }

  async #createAsset(request: Request): Promise<Response> {
    const user = await this.#authenticatedUser(request);
    if (!user) return json({ error: "authentication required" }, 401);
    const body = await readJson(request);
    if (!body.ok) return json({ error: body.error }, body.status);
    const parsed = parseCreateAsset(body.value);
    if (!parsed.ok) return json({ error: parsed.error }, 400);

    const id = newId();
    const shareId = newId();
    const prefix = `users/${user.id}/assets/${id}`;
    const originalKey = `${prefix}/original`;
    const previewKey = parsed.value.preview ? `${prefix}/preview` : null;
    const uploadExpiresAt = new Date(Date.now() + UPLOAD_TTL_MS);
    let asset: AssetRecord;
    try {
      asset = await this.#database.reserveAsset({
        id,
        shareId,
        userId: user.id,
        storageBackend: this.#config.storage.backend,
        storageBucket: this.#config.storage.bucket,
        originalKey,
        previewKey,
        asset: parsed.value,
        uploadExpiresAt,
        multipartUploadId: null,
        multipartPartSize: null,
      });
    } catch (error) {
      if (error instanceof Error && error.message === "quota_exceeded") {
        return json({ error: "account storage quota exceeded" }, 409);
      }
      throw error;
    }

    const originalTarget = targetFor(asset, false);
    let originalUpload;
    try {
      originalUpload = asset.originalBytes <= SINGLE_UPLOAD_MAX_BYTES
        ? await this.#storage.createSingleUpload(originalTarget)
        : await this.#storage.createMultipartUpload(originalTarget);
      if (originalUpload.type === "multipart") {
        await this.#database.setMultipartUpload(
          user.id,
          asset.id,
          originalUpload.uploadId,
          originalUpload.partSize,
        );
      }
      const previewUpload = previewKey
        ? await this.#storage.createSingleUpload(targetFor(asset, true))
        : null;
      return json(
        {
          asset: ownerAssetJson(asset, this.#config.publicOrigin),
          upload_expires_at: uploadExpiresAt.toISOString(),
          original_upload: originalUpload,
          preview_upload: previewUpload,
        },
        201,
      );
    } catch (error) {
      if (originalUpload?.type === "multipart") {
        await this.#storage
          .abortMultipartUpload(originalKey, originalUpload.uploadId)
          .catch(() => {});
      }
      await this.#database.markAssetDeleted(user.id, asset.id).catch(() => {});
      throw error;
    }
  }

  async #refreshParts(request: Request, assetId: string): Promise<Response> {
    const user = await this.#authenticatedUser(request);
    if (!user) return json({ error: "authentication required" }, 401);
    const asset = await this.#database.getOwnerAsset(user.id, assetId);
    if (
      !asset ||
      asset.status !== "pending" ||
      !asset.multipartUploadId ||
      asset.uploadExpiresAt.getTime() <= Date.now()
    ) {
      return json({ error: "upload is unavailable" }, 404);
    }
    const body = await readJson(request);
    if (!body.ok) return json({ error: body.error }, body.status);
    const values = record(body.value)?.part_numbers;
    const partNumbers = Array.isArray(values) ? values.map(Number) : [];
    try {
      const parts = await this.#storage.refreshMultipartParts(
        targetFor(asset, false),
        asset.multipartUploadId,
        partNumbers,
      );
      return json({ parts });
    } catch {
      return json({ error: "part_numbers are invalid" }, 400);
    }
  }

  async #completeAsset(request: Request, assetId: string): Promise<Response> {
    const user = await this.#authenticatedUser(request);
    if (!user) return json({ error: "authentication required" }, 401);
    const asset = await this.#database.getOwnerAsset(user.id, assetId);
    if (asset?.status === "ready") {
      return json({ asset: ownerAssetJson(asset, this.#config.publicOrigin) });
    }
    if (!asset || asset.status !== "pending" || asset.uploadExpiresAt.getTime() <= Date.now()) {
      return json({ error: "upload is unavailable" }, 404);
    }
    const body = await readJson(request);
    if (!body.ok) return json({ error: body.error }, body.status);
    const parts = parseParts(record(body.value)?.parts);
    if (asset.multipartUploadId) {
      const expectedParts = Math.ceil(asset.originalBytes / (asset.multipartPartSize ?? MULTIPART_PART_BYTES));
      if (!parts || parts.length !== expectedParts) {
        return json({ error: `exactly ${expectedParts} completed parts are required` }, 400);
      }
      try {
        await this.#storage.completeMultipartUpload(
          asset.originalKey,
          asset.multipartUploadId,
          parts,
        );
      } catch (error) {
        // Completion may have succeeded even if its response was lost. If the
        // final object is already valid, continue so a client retry can heal
        // the database state instead of becoming stuck on NoSuchUpload.
        try {
          await this.#verifyStoredAsset(asset);
        } catch {
          console.warn("multipart completion failed", asset.id, error);
          return json({ error: "object storage could not complete the upload" }, 502);
        }
      }
    } else if (parts) {
      return json({ error: "parts are only valid for multipart uploads" }, 400);
    }

    try {
      await this.#verifyStoredAsset(asset);
    } catch (error) {
      console.warn("rejecting invalid uploaded asset", asset.id, error);
      try {
        await this.#storage.deleteObjects(objectKeys(asset));
        await this.#database.markAssetDeleted(user.id, asset.id);
      } catch (cleanupError) {
        console.warn("invalid upload cleanup will retry after expiry", asset.id, cleanupError);
      }
      return json({ error: "uploaded object did not match its reservation" }, 422);
    }

    try {
      const ready = await this.#database.markAssetReady(user.id, asset.id);
      return json({ asset: ownerAssetJson(ready, this.#config.publicOrigin) });
    } catch (error) {
      // The object is valid, so leave the pending row intact. A retry can
      // reconcile a transient database failure without uploading it again.
      const current = await this.#database.getOwnerAsset(user.id, asset.id).catch(() => null);
      if (current?.status === "ready") {
        return json({ asset: ownerAssetJson(current, this.#config.publicOrigin) });
      }
      console.error("failed to finalize valid uploaded asset", asset.id, error);
      return json({ error: "upload finalization is temporarily unavailable" }, 503);
    }
  }

  async #updateShare(request: Request, assetId: string): Promise<Response> {
    const user = await this.#authenticatedUser(request);
    if (!user) return json({ error: "authentication required" }, 401);
    const body = await readJson(request);
    if (!body.ok) return json({ error: body.error }, body.status);
    const input = record(body.value);
    const access = input?.access;
    if (access !== "private" && access !== "shared") {
      return json({ error: "access must be private or shared" }, 400);
    }
    const expiresAt = parseShareExpiry(input);
    if (expiresAt === undefined) return json({ error: "share expiry is invalid" }, 400);
    const asset = await this.#database.updateAssetAccess(
      user.id,
      assetId,
      access,
      expiresAt,
    );
    return asset
      ? json({ asset: ownerAssetJson(asset, this.#config.publicOrigin) })
      : json({ error: "asset not found" }, 404);
  }

  async #rotateShare(request: Request, assetId: string): Promise<Response> {
    const user = await this.#authenticatedUser(request);
    if (!user) return json({ error: "authentication required" }, 401);
    const asset = await this.#database.rotateShareId(user.id, assetId, newId());
    return asset
      ? json({ asset: ownerAssetJson(asset, this.#config.publicOrigin) })
      : json({ error: "asset not found" }, 404);
  }

  async #ownerMedia(request: Request, assetId: string): Promise<Response> {
    const user = await this.#authenticatedUser(request);
    if (!user) return json({ error: "authentication required" }, 401);
    const asset = await this.#database.getOwnerAsset(user.id, assetId);
    if (!asset || asset.status !== "ready") return json({ error: "asset not found" }, 404);
    const preview = new URL(request.url).searchParams.get("variant") === "preview";
    if (preview && !asset.previewKey) return json({ error: "preview not found" }, 404);
    const url = await this.#storage.createDownloadUrl(
      preview ? asset.previewKey! : asset.originalKey,
      preview ? asset.previewMimeType! : asset.originalMimeType,
    );
    return json({ url });
  }

  async #sharedAsset(_request: Request, shareId: string): Promise<Response> {
    const asset = await this.#database.getSharedAsset(shareId);
    if (!asset) return json({ error: "share not found" }, 404);
    if (asset.sharePasswordHash) {
      return json({ password_required: true }, 401);
    }
    const [mediaUrl, previewUrl] = await Promise.all([
      this.#storage.createDownloadUrl(asset.originalKey, asset.originalMimeType),
      asset.previewKey
        ? this.#storage.createDownloadUrl(asset.previewKey, asset.previewMimeType!)
        : Promise.resolve(null),
    ]);
    return json({
      asset: {
        share_id: asset.shareId,
        title: asset.title,
        kind: asset.kind,
        mime_type: asset.originalMimeType,
        bytes: asset.originalBytes,
        width: asset.width,
        height: asset.height,
        duration_ms: asset.durationMs,
        created_at: asset.createdAt.toISOString(),
        expires_at: asset.shareExpiresAt?.toISOString() ?? null,
        media_url: mediaUrl,
        preview_url: previewUrl,
      },
    });
  }

  async #deleteAsset(request: Request, assetId: string): Promise<Response> {
    const user = await this.#authenticatedUser(request);
    if (!user) return json({ error: "authentication required" }, 401);
    const asset = await this.#database.getOwnerAsset(user.id, assetId);
    if (!asset) return json({ error: "asset not found" }, 404);
    if (asset.multipartUploadId) {
      await this.#storage
        .abortMultipartUpload(asset.originalKey, asset.multipartUploadId)
        .catch(() => {});
    }
    await this.#storage.deleteObjects(objectKeys(asset));
    await this.#database.markAssetDeleted(user.id, asset.id);
    return new Response(null, { status: 204, headers: noStoreHeaders() });
  }

  async #receiveEmailEvent(request: Request): Promise<Response> {
    if (!this.#config.mail.snsTopicArn) return json({ error: "not found" }, 404);
    const body = await readJson(request);
    if (!body.ok) return json({ error: body.error }, body.status);
    try {
      const envelope = await verifySnsEnvelope(
        body.value,
        this.#config.mail.snsTopicArn,
        this.#fetch,
      );
      if (envelope.Type === "SubscriptionConfirmation") {
        await confirmSnsSubscription(envelope, this.#fetch);
      }
      for (const event of extractSuppressionEvents(envelope)) {
        await this.#database.upsertEmailSuppression(event);
      }
      return json({ ok: true }, 202);
    } catch (error) {
      console.warn("rejected SES SNS event", error);
      return json({ error: "invalid notification" }, 403);
    }
  }

  async #authenticatedUser(request: Request): Promise<SessionUser | null> {
    const token = sessionToken(request);
    return token ? this.#database.userForAccessToken(hashToken(token)) : null;
  }

  async #issueSession(
    client: Parameters<Parameters<SharingDatabase["withTransaction"]>[0]>[0],
    user: SessionUser,
    kind: ClientKind,
  ): Promise<{
    user: SessionUser;
    accessToken: string;
    refreshToken?: string;
    accessExpiresIn: number;
  }> {
    const accessToken = newOpaqueToken();
    const refreshToken = kind === "desktop" ? newOpaqueToken() : undefined;
    const accessExpiresIn = kind === "desktop"
      ? DESKTOP_ACCESS_TTL_MS / 1_000
      : WEB_SESSION_TTL_SECONDS;
    await this.#database.insertSession(client, {
      id: newId(),
      userId: user.id,
      kind,
      accessHash: hashToken(accessToken),
      refreshHash: refreshToken ? hashToken(refreshToken) : undefined,
      accessExpiresAt: new Date(Date.now() + accessExpiresIn * 1_000),
      refreshExpiresAt: refreshToken
        ? new Date(Date.now() + DESKTOP_REFRESH_TTL_MS)
        : undefined,
    });
    return { user, accessToken, refreshToken, accessExpiresIn };
  }

  async #interactiveAuthAllowed(
    email: string,
    ip: string,
    _input: Record<string, unknown> | undefined,
  ): Promise<boolean> {
    return this.#config.auth.allowedEmails.has(email) &&
      clientIpAllowed(ip, this.#config.auth.allowedCidrs);
  }

  async #verifyStoredAsset(asset: AssetRecord): Promise<void> {
    const original = await this.#storage.inspectObject(asset.originalKey);
    if (
      original.bytes !== asset.originalBytes ||
      original.mimeType !== asset.originalMimeType ||
      original.sha256 !== asset.originalSha256 ||
      !(await expectedMimeMatches(asset.kind, asset.originalMimeType, original.firstBytes))
    ) {
      throw new Error("original object mismatch");
    }
    if (asset.previewKey) {
      const preview = await this.#storage.inspectObject(asset.previewKey);
      if (
        preview.bytes !== asset.previewBytes ||
        preview.mimeType !== asset.previewMimeType ||
        preview.sha256 !== asset.previewSha256 ||
        !(await previewMimeMatches(asset.previewMimeType!, preview.firstBytes))
      ) {
        throw new Error("preview object mismatch");
      }
    }
  }

  async #cleanupExpiredUploads(): Promise<void> {
    try {
      await this.#database.withCleanupLock(async () => {
        await this.#database.deleteExpiredAuthRecords();
        while (true) {
          const expired = await this.#database.expiredPendingAssets();
          let storageFailure = false;
          for (const asset of expired) {
            if (asset.multipartUploadId) {
              await this.#storage
                .abortMultipartUpload(asset.originalKey, asset.multipartUploadId)
                .catch((error) => console.warn("failed to abort stale multipart upload", error));
            }
            try {
              await this.#storage.deleteObjects(objectKeys(asset));
            } catch (error) {
              console.warn("failed to remove stale upload objects", error);
              storageFailure = true;
              continue;
            }
            await this.#database.markAssetDeleted(asset.ownerId, asset.id);
          }
          if (expired.length < 100 || storageFailure) break;
        }
      });
    } catch (error) {
      console.warn("stale upload cleanup failed", error);
    }
  }
}

function targetFor(asset: AssetRecord, preview: boolean): UploadTarget {
  return preview
    ? {
        key: asset.previewKey!,
        bytes: asset.previewBytes,
        mimeType: asset.previewMimeType!,
        sha256: asset.previewSha256!,
      }
    : {
        key: asset.originalKey,
        bytes: asset.originalBytes,
        mimeType: asset.originalMimeType,
        sha256: asset.originalSha256,
      };
}

function objectKeys(asset: AssetRecord): string[] {
  return [asset.originalKey, asset.previewKey].filter((key): key is string => Boolean(key));
}

function ownerAssetJson(asset: AssetRecord, origin: string): Record<string, unknown> {
  return {
    id: asset.id,
    share_id: asset.shareId,
    share_url: `${origin}/${asset.shareId}`,
    title: asset.title,
    kind: asset.kind,
    status: asset.status,
    access: asset.access,
    original_mime_type: asset.originalMimeType,
    original_bytes: asset.originalBytes,
    preview_mime_type: asset.previewMimeType,
    preview_bytes: asset.previewBytes,
    width: asset.width,
    height: asset.height,
    duration_ms: asset.durationMs,
    upload_expires_at: asset.uploadExpiresAt.toISOString(),
    share_expires_at: asset.shareExpiresAt?.toISOString() ?? null,
    password_protected: Boolean(asset.sharePasswordHash),
    created_at: asset.createdAt.toISOString(),
    updated_at: asset.updatedAt.toISOString(),
  };
}

function parseParts(value: unknown): MultipartPart[] | null {
  if (value === undefined || value === null) return null;
  if (!Array.isArray(value) || value.length === 0) return null;
  const parts = value.map((item) => {
    const entry = record(item);
    return {
      partNumber: Number(entry?.part_number),
      etag: string(entry?.etag)?.trim() ?? "",
    };
  });
  if (
    parts.some(
      ({ partNumber, etag }) => !Number.isInteger(partNumber) || partNumber < 1 || !etag,
    ) ||
    parts.some(({ partNumber }, index) => partNumber !== index + 1)
  ) {
    return null;
  }
  return parts;
}

function sessionResponse(
  issued: {
    user: SessionUser;
    accessToken: string;
    refreshToken?: string;
    accessExpiresIn: number;
  },
  kind: ClientKind,
): Response {
  if (kind === "desktop") {
    return json({
      user: issued.user,
      access_token: issued.accessToken,
      refresh_token: issued.refreshToken,
      expires_in: issued.accessExpiresIn,
    });
  }
  const response = json({ user: issued.user });
  response.headers.set("Set-Cookie", webSessionCookie(issued.accessToken, WEB_SESSION_TTL_SECONDS));
  return response;
}

function genericLoginStart(retryAfterSeconds?: number): Response {
  const response = json({ ok: true }, 202);
  if (retryAfterSeconds) response.headers.set("Retry-After", String(retryAfterSeconds));
  return response;
}

async function readJson(
  request: Request,
): Promise<{ ok: true; value: unknown } | { ok: false; error: string; status: number }> {
  const declared = Number(request.headers.get("Content-Length"));
  if (Number.isFinite(declared) && declared > JSON_BODY_LIMIT) {
    return { ok: false, error: "request body is too large", status: 413 };
  }
  try {
    const text = await request.text();
    if (Buffer.byteLength(text, "utf8") > JSON_BODY_LIMIT) {
      return { ok: false, error: "request body is too large", status: 413 };
    }
    return { ok: true, value: JSON.parse(text) };
  } catch {
    return { ok: false, error: "request body must be valid JSON", status: 400 };
  }
}

function methodNotAllowed(methods: string[]): Response {
  const response = json({ error: "method not allowed" }, 405);
  response.headers.set("Allow", methods.join(", "));
  return response;
}

function json(body: unknown, status = 200): Response {
  const headers = noStoreHeaders();
  headers.set("Content-Type", "application/json; charset=utf-8");
  return new Response(JSON.stringify(body), { status, headers });
}

function noStoreHeaders(): Headers {
  return new Headers({ "Cache-Control": "no-store" });
}

function redirect(location: string, headers: Record<string, string> = {}): Response {
  return new Response(null, {
    status: 302,
    headers: { Location: location, "Cache-Control": "no-store", ...headers },
  });
}

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function string(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function validEmail(value: string): boolean {
  return value.length <= 254 && /^[^\s@]+@[^\s@]+\.[^\s@]+$/u.test(value);
}

function parseClientKind(value: unknown): ClientKind | null {
  if (value === undefined || value === "web") return "web";
  return value === "desktop" ? "desktop" : null;
}

interface OauthState {
  state: string;
  verifier: string;
  expiresAt: number;
}

function signOauthState(key: Buffer, value: OauthState): string {
  const payload = Buffer.from(JSON.stringify(value)).toString("base64url");
  const signature = createHmac("sha256", key).update(payload).digest("base64url");
  return `${payload}.${signature}`;
}

function verifyOauthState(key: Buffer, value: string): OauthState | null {
  const [payload, signature, extra] = value.split(".");
  if (!payload || !signature || extra) return null;
  const expected = createHmac("sha256", key).update(payload).digest();
  let actual: Buffer;
  try {
    actual = Buffer.from(signature, "base64url");
  } catch {
    return null;
  }
  if (!safeEqual(actual, expected)) return null;
  try {
    const parsed = JSON.parse(Buffer.from(payload, "base64url").toString("utf8"));
    return typeof parsed?.state === "string" &&
      typeof parsed?.verifier === "string" &&
      typeof parsed?.expiresAt === "number"
      ? parsed
      : null;
  } catch {
    return null;
  }
}
