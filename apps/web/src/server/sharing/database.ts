import pg from "pg";
import { SHARING_MIGRATIONS } from "./migrations.ts";
import type {
  AssetAccess,
  AssetRecord,
  ClientKind,
  CreateAssetInput,
  SessionUser,
} from "./types.ts";

const { Client, Pool } = pg;
const MIGRATION_LOCK_ID = 1_913_066_776;

type Queryable = Pick<pg.Pool, "query"> | pg.PoolClient;

interface LoginCodeRecord {
  id: string;
  codeHmac: Buffer;
  attempts: number;
  expiresAt: Date;
}

function number(value: unknown): number {
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) throw new Error("database integer exceeds JavaScript range");
  return parsed;
}

function mapUser(row: Record<string, unknown>): SessionUser {
  return {
    id: String(row.id),
    email: String(row.email),
    quotaBytes: number(row.quota_bytes),
    usedBytes: number(row.used_bytes),
    reservedBytes: number(row.reserved_bytes),
  };
}

function mapAsset(row: Record<string, unknown>): AssetRecord {
  return {
    id: String(row.id),
    ownerId: String(row.owner_id),
    shareId: String(row.share_id),
    title: row.title === null ? null : String(row.title),
    kind: row.kind as AssetRecord["kind"],
    status: row.status as AssetRecord["status"],
    access: row.access as AssetAccess,
    storageBackend: String(row.storage_backend),
    storageBucket: String(row.storage_bucket),
    originalKey: String(row.original_key),
    previewKey: row.preview_key === null ? null : String(row.preview_key),
    originalMimeType: String(row.original_mime_type),
    previewMimeType:
      row.preview_mime_type === null ? null : String(row.preview_mime_type),
    originalBytes: number(row.original_bytes),
    previewBytes: number(row.preview_bytes),
    reservedBytes: number(row.reserved_bytes),
    originalSha256: String(row.original_sha256),
    previewSha256: row.preview_sha256 === null ? null : String(row.preview_sha256),
    width: row.width === null ? null : number(row.width),
    height: row.height === null ? null : number(row.height),
    durationMs: row.duration_ms === null ? null : number(row.duration_ms),
    multipartUploadId:
      row.multipart_upload_id === null ? null : String(row.multipart_upload_id),
    multipartPartSize:
      row.multipart_part_size === null ? null : number(row.multipart_part_size),
    uploadExpiresAt: new Date(String(row.upload_expires_at)),
    shareExpiresAt:
      row.share_expires_at === null ? null : new Date(String(row.share_expires_at)),
    sharePasswordHash:
      row.share_password_hash === null ? null : String(row.share_password_hash),
    shareAccessVersion: number(row.share_access_version),
    createdAt: new Date(String(row.created_at)),
    updatedAt: new Date(String(row.updated_at)),
  };
}

export class SharingDatabase {
  readonly #runtimeUrl: string;
  readonly #migrationUrl: string;
  #pool?: pg.Pool;

  constructor(runtimeUrl: string, migrationUrl: string) {
    this.#runtimeUrl = runtimeUrl;
    this.#migrationUrl = migrationUrl;
  }

  async start(): Promise<void> {
    await runMigrations(this.#migrationUrl);
    this.#pool = new Pool({
      connectionString: this.#runtimeUrl,
      max: 5,
      idleTimeoutMillis: 30_000,
      connectionTimeoutMillis: 10_000,
      application_name: "captures-web",
    });
    await this.#pool.query("SELECT 1");
  }

  async close(): Promise<void> {
    await this.#pool?.end();
    this.#pool = undefined;
  }

  #queryable(): pg.Pool {
    if (!this.#pool) throw new Error("sharing database is not started");
    return this.#pool;
  }

  async withTransaction<T>(work: (client: pg.PoolClient) => Promise<T>): Promise<T> {
    const client = await this.#queryable().connect();
    try {
      await client.query("BEGIN");
      const result = await work(client);
      await client.query("COMMIT");
      return result;
    } catch (error) {
      await client.query("ROLLBACK");
      throw error;
    } finally {
      client.release();
    }
  }

  async isEmailSuppressed(email: string): Promise<boolean> {
    const result = await this.#queryable().query(
      "SELECT 1 FROM email_suppressions WHERE email = $1",
      [email],
    );
    return result.rowCount === 1;
  }

  async reserveLoginCode(input: {
    id: string;
    email: string;
    clientKind: ClientKind;
    codeHmac: Buffer;
    ipHmac: Buffer;
    expiresAt: Date;
  }): Promise<{ allowed: boolean; retryAfterSeconds: number }> {
    return this.withTransaction(async (client) => {
      // Login starts are tiny and low-volume. One transaction-scoped lock makes
      // the database limits exact across both app replicas instead of allowing
      // concurrent check-then-insert races.
      await client.query("SELECT pg_advisory_xact_lock($1)", [MIGRATION_LOCK_ID + 2]);
      const result = await client.query<{
        cooldown_seconds: string | null;
        email_hour: string;
        ip_hour: string;
        global_ten_minutes: string;
      }>(
        `SELECT
          EXTRACT(EPOCH FROM (MAX(created_at) + interval '60 seconds' - now()))
            FILTER (WHERE email = $1 AND created_at > now() - interval '60 seconds') AS cooldown_seconds,
          COUNT(*) FILTER (WHERE email = $1 AND created_at > now() - interval '1 hour') AS email_hour,
          COUNT(*) FILTER (WHERE request_ip_hmac = $2 AND created_at > now() - interval '1 hour') AS ip_hour,
          COUNT(*) FILTER (WHERE created_at > now() - interval '10 minutes') AS global_ten_minutes
         FROM login_codes
         WHERE created_at > now() - interval '1 hour'`,
        [input.email, input.ipHmac],
      );
      const row = result.rows[0];
      const cooldown = Math.max(0, Math.ceil(Number(row?.cooldown_seconds ?? 0)));
      const allowed =
        cooldown === 0 &&
        Number(row?.email_hour ?? 0) < 5 &&
        Number(row?.ip_hour ?? 0) < 20 &&
        Number(row?.global_ten_minutes ?? 0) < 200;
      if (!allowed) return { allowed: false, retryAfterSeconds: cooldown || 60 };

      await client.query(
        `INSERT INTO login_codes
          (id, email, client_kind, code_hmac, request_ip_hmac, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)`,
        [
          input.id,
          input.email,
          input.clientKind,
          input.codeHmac,
          input.ipHmac,
          input.expiresAt,
        ],
      );
      return { allowed: true, retryAfterSeconds: 0 };
    });
  }

  async attachSesMessageId(id: string, messageId: string): Promise<void> {
    await this.#queryable().query(
      "UPDATE login_codes SET ses_message_id = $2 WHERE id = $1",
      [id, messageId],
    );
  }

  async invalidateLoginCode(id: string): Promise<void> {
    await this.#queryable().query(
      "UPDATE login_codes SET consumed_at = now() WHERE id = $1",
      [id],
    );
  }

  async latestLoginCode(
    client: pg.PoolClient,
    email: string,
    clientKind: ClientKind,
  ): Promise<LoginCodeRecord | null> {
    const result = await client.query(
      `SELECT id, code_hmac, attempts, expires_at
       FROM login_codes
       WHERE email = $1 AND client_kind = $2 AND consumed_at IS NULL
       ORDER BY created_at DESC
       LIMIT 1
       FOR UPDATE`,
      [email, clientKind],
    );
    const row = result.rows[0] as Record<string, unknown> | undefined;
    return row
      ? {
          id: String(row.id),
          codeHmac: row.code_hmac as Buffer,
          attempts: number(row.attempts),
          expiresAt: new Date(String(row.expires_at)),
        }
      : null;
  }

  async recordFailedLoginAttempt(client: Queryable, id: string): Promise<void> {
    await client.query(
      "UPDATE login_codes SET attempts = LEAST(attempts + 1, 6) WHERE id = $1",
      [id],
    );
  }

  async consumeLoginCode(client: Queryable, id: string): Promise<void> {
    await client.query(
      "UPDATE login_codes SET consumed_at = now() WHERE id = $1",
      [id],
    );
  }

  async upsertUser(client: Queryable, id: string, email: string): Promise<SessionUser> {
    const result = await client.query(
      `INSERT INTO users (id, email)
       VALUES ($1, $2)
       ON CONFLICT (email) DO UPDATE SET updated_at = now()
       RETURNING id, email, quota_bytes, used_bytes, reserved_bytes, suspended_at`,
      [id, email],
    );
    const row = result.rows[0] as Record<string, unknown>;
    if (row.suspended_at) throw new Error("account_suspended");
    return mapUser(row);
  }

  async upsertGoogleUser(
    client: Queryable,
    id: string,
    email: string,
    googleSubject: string,
  ): Promise<SessionUser> {
    const result = await client.query(
      `INSERT INTO users (id, email, google_subject)
       VALUES ($1, $2, $3)
       ON CONFLICT (email) DO UPDATE SET
         google_subject = CASE
           WHEN users.google_subject IS NULL OR users.google_subject = EXCLUDED.google_subject
           THEN EXCLUDED.google_subject
           ELSE users.google_subject
         END,
         updated_at = now()
       RETURNING id, email, google_subject, quota_bytes, used_bytes,
                 reserved_bytes, suspended_at`,
      [id, email, googleSubject],
    );
    const row = result.rows[0] as Record<string, unknown>;
    if (row.google_subject !== googleSubject) throw new Error("google_identity_conflict");
    if (row.suspended_at) throw new Error("account_suspended");
    return mapUser(row);
  }

  async insertSession(
    client: Queryable,
    input: {
      id: string;
      userId: string;
      kind: ClientKind;
      accessHash: Buffer;
      refreshHash?: Buffer;
      accessExpiresAt: Date;
      refreshExpiresAt?: Date;
    },
  ): Promise<void> {
    await client.query(
      `INSERT INTO sessions
        (id, user_id, kind, access_token_hash, refresh_token_hash,
         access_expires_at, refresh_expires_at)
       VALUES ($1, $2, $3, $4, $5, $6, $7)`,
      [
        input.id,
        input.userId,
        input.kind,
        input.accessHash,
        input.refreshHash ?? null,
        input.accessExpiresAt,
        input.refreshExpiresAt ?? null,
      ],
    );
  }

  async userForAccessToken(accessHash: Buffer): Promise<SessionUser | null> {
    const result = await this.#queryable().query(
      `UPDATE sessions s
       SET last_used_at = now()
       FROM users u
       WHERE s.access_token_hash = $1
         AND s.user_id = u.id
         AND s.revoked_at IS NULL
         AND s.access_expires_at > now()
         AND u.suspended_at IS NULL
       RETURNING u.id, u.email, u.quota_bytes, u.used_bytes, u.reserved_bytes`,
      [accessHash],
    );
    return result.rows[0] ? mapUser(result.rows[0]) : null;
  }

  async rotateDesktopSession(
    refreshHash: Buffer,
    accessHash: Buffer,
    accessExpiresAt: Date,
  ): Promise<SessionUser | null> {
    const result = await this.#queryable().query(
      `UPDATE sessions s
       SET access_token_hash = $2, access_expires_at = $3, last_used_at = now()
       FROM users u
       WHERE s.refresh_token_hash = $1
         AND s.user_id = u.id
         AND s.kind = 'desktop'
         AND s.revoked_at IS NULL
         AND s.refresh_expires_at > now()
         AND u.suspended_at IS NULL
       RETURNING u.id, u.email, u.quota_bytes, u.used_bytes, u.reserved_bytes`,
      [refreshHash, accessHash, accessExpiresAt],
    );
    return result.rows[0] ? mapUser(result.rows[0]) : null;
  }

  async revokeSession(accessHash: Buffer): Promise<void> {
    await this.#queryable().query(
      "UPDATE sessions SET revoked_at = now() WHERE access_token_hash = $1",
      [accessHash],
    );
  }

  async reserveAsset(input: {
    id: string;
    shareId: string;
    userId: string;
    storageBackend: string;
    storageBucket: string;
    originalKey: string;
    previewKey: string | null;
    asset: CreateAssetInput;
    uploadExpiresAt: Date;
    multipartUploadId: string | null;
    multipartPartSize: number | null;
  }): Promise<AssetRecord> {
    return this.withTransaction(async (client) => {
      const reservedBytes = input.asset.original.bytes + (input.asset.preview?.bytes ?? 0);
      const userResult = await client.query(
        `SELECT id, quota_bytes, used_bytes, reserved_bytes, suspended_at
         FROM users WHERE id = $1 FOR UPDATE`,
        [input.userId],
      );
      const user = userResult.rows[0] as Record<string, unknown> | undefined;
      if (!user || user.suspended_at) throw new Error("account_unavailable");
      if (
        number(user.used_bytes) + number(user.reserved_bytes) + reservedBytes >
        number(user.quota_bytes)
      ) {
        throw new Error("quota_exceeded");
      }

      const result = await client.query(
        `INSERT INTO assets (
          id, owner_id, share_id, title, kind, storage_backend, storage_bucket,
          original_key, preview_key, original_mime_type, preview_mime_type,
          original_bytes, preview_bytes, reserved_bytes, original_sha256,
          preview_sha256, width, height, duration_ms, multipart_upload_id,
          multipart_part_size, upload_expires_at
        ) VALUES (
          $1, $2, $3, $4, $5, $6, $7,
          $8, $9, $10, $11, $12, $13, $14, $15,
          $16, $17, $18, $19, $20, $21, $22
        ) RETURNING *`,
        [
          input.id,
          input.userId,
          input.shareId,
          input.asset.title ?? null,
          input.asset.kind,
          input.storageBackend,
          input.storageBucket,
          input.originalKey,
          input.previewKey,
          input.asset.original.mimeType,
          input.asset.preview?.mimeType ?? null,
          input.asset.original.bytes,
          input.asset.preview?.bytes ?? 0,
          reservedBytes,
          input.asset.original.sha256,
          input.asset.preview?.sha256 ?? null,
          input.asset.width ?? null,
          input.asset.height ?? null,
          input.asset.durationMs ?? null,
          input.multipartUploadId,
          input.multipartPartSize,
          input.uploadExpiresAt,
        ],
      );
      await client.query(
        "UPDATE users SET reserved_bytes = reserved_bytes + $2, updated_at = now() WHERE id = $1",
        [input.userId, reservedBytes],
      );
      return mapAsset(result.rows[0]);
    });
  }

  async setMultipartUpload(
    userId: string,
    assetId: string,
    uploadId: string,
    partSize: number,
  ): Promise<void> {
    const result = await this.#queryable().query(
      `UPDATE assets
       SET multipart_upload_id = $3, multipart_part_size = $4, updated_at = now()
       WHERE id = $1 AND owner_id = $2 AND status = 'pending' AND deleted_at IS NULL`,
      [assetId, userId, uploadId, partSize],
    );
    if (result.rowCount !== 1) throw new Error("asset_not_pending");
  }

  async getOwnerAsset(userId: string, assetId: string): Promise<AssetRecord | null> {
    const result = await this.#queryable().query(
      "SELECT * FROM assets WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL",
      [assetId, userId],
    );
    return result.rows[0] ? mapAsset(result.rows[0]) : null;
  }

  async listAssets(
    userId: string,
    cursorAssetId: string | null,
    limit: number,
  ): Promise<AssetRecord[]> {
    const result = await this.#queryable().query(
      `SELECT * FROM assets
       WHERE owner_id = $1
         AND deleted_at IS NULL
         AND (
           $2::text IS NULL
           OR (created_at, id) < (
             SELECT created_at, id
             FROM assets
             WHERE owner_id = $1 AND id = $2
           )
         )
       ORDER BY created_at DESC, id DESC
       LIMIT $3`,
      [userId, cursorAssetId, limit],
    );
    return result.rows.map(mapAsset);
  }

  async markAssetReady(userId: string, assetId: string): Promise<AssetRecord> {
    return this.withTransaction(async (client) => {
      const result = await client.query(
        `UPDATE assets
         SET status = 'ready', multipart_upload_id = NULL, multipart_part_size = NULL,
             updated_at = now()
         WHERE id = $1 AND owner_id = $2 AND status = 'pending' AND deleted_at IS NULL
         RETURNING *`,
        [assetId, userId],
      );
      if (!result.rows[0]) throw new Error("asset_not_pending");
      const asset = mapAsset(result.rows[0]);
      await client.query(
        `UPDATE users
         SET reserved_bytes = reserved_bytes - $2,
             used_bytes = used_bytes + $2,
             updated_at = now()
         WHERE id = $1`,
        [userId, asset.reservedBytes],
      );
      return asset;
    });
  }

  async updateAssetAccess(
    userId: string,
    assetId: string,
    access: AssetAccess,
    expiresAt: Date | null,
  ): Promise<AssetRecord | null> {
    const result = await this.#queryable().query(
      `UPDATE assets
       SET access = $3,
           share_expires_at = CASE WHEN $3 = 'shared' THEN $4 ELSE NULL END,
           share_access_version = share_access_version + 1,
           updated_at = now()
       WHERE id = $1 AND owner_id = $2 AND status = 'ready' AND deleted_at IS NULL
       RETURNING *`,
      [assetId, userId, access, expiresAt],
    );
    return result.rows[0] ? mapAsset(result.rows[0]) : null;
  }

  async rotateShareId(
    userId: string,
    assetId: string,
    shareId: string,
  ): Promise<AssetRecord | null> {
    const result = await this.#queryable().query(
      `UPDATE assets
       SET share_id = $3, share_access_version = share_access_version + 1, updated_at = now()
       WHERE id = $1 AND owner_id = $2 AND status = 'ready' AND deleted_at IS NULL
       RETURNING *`,
      [assetId, userId, shareId],
    );
    return result.rows[0] ? mapAsset(result.rows[0]) : null;
  }

  async getSharedAsset(shareId: string): Promise<AssetRecord | null> {
    const result = await this.#queryable().query(
      `SELECT * FROM assets
       WHERE share_id = $1
         AND status = 'ready'
         AND access = 'shared'
         AND deleted_at IS NULL
         AND (share_expires_at IS NULL OR share_expires_at > now())`,
      [shareId],
    );
    return result.rows[0] ? mapAsset(result.rows[0]) : null;
  }

  async markAssetDeleted(userId: string, assetId: string): Promise<AssetRecord | null> {
    return this.withTransaction(async (client) => {
      const existingResult = await client.query(
        `SELECT * FROM assets
         WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL
         FOR UPDATE`,
        [assetId, userId],
      );
      if (!existingResult.rows[0]) return null;
      const existing = mapAsset(existingResult.rows[0]);
      const result = await client.query(
        `UPDATE assets
         SET status = 'deleting', access = 'private', deleted_at = now(),
             share_access_version = share_access_version + 1, updated_at = now()
         WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL
         RETURNING *`,
        [assetId, userId],
      );
      const asset = mapAsset(result.rows[0]);
      const usedDelta = existing.status === "ready" ? existing.reservedBytes : 0;
      const reservedDelta = existing.status === "pending" ? existing.reservedBytes : 0;
      await client.query(
        `UPDATE users
         SET used_bytes = GREATEST(0, used_bytes - $2),
             reserved_bytes = GREATEST(0, reserved_bytes - $3),
             updated_at = now()
         WHERE id = $1`,
        [userId, usedDelta, reservedDelta],
      );
      return asset;
    });
  }

  async expiredPendingAssets(limit = 100): Promise<AssetRecord[]> {
    const result = await this.#queryable().query(
      `SELECT * FROM assets
       WHERE status = 'pending' AND deleted_at IS NULL AND upload_expires_at < now()
       ORDER BY upload_expires_at
       LIMIT $1`,
      [limit],
    );
    return result.rows.map(mapAsset);
  }

  async deleteExpiredAuthRecords(): Promise<void> {
    await this.#queryable().query(
      `DELETE FROM login_codes
       WHERE created_at < now() - interval '7 days'`,
    );
    await this.#queryable().query(
      `DELETE FROM sessions
       WHERE (kind = 'web' AND access_expires_at < now() - interval '7 days')
          OR (kind = 'desktop' AND refresh_expires_at < now() - interval '7 days')
          OR (revoked_at IS NOT NULL AND revoked_at < now() - interval '7 days')`,
    );
  }

  async withCleanupLock(work: () => Promise<void>): Promise<boolean> {
    const client = await this.#queryable().connect();
    try {
      const result = await client.query<{ acquired: boolean }>(
        "SELECT pg_try_advisory_lock($1) AS acquired",
        [MIGRATION_LOCK_ID + 1],
      );
      if (result.rows[0]?.acquired !== true) return false;
      try {
        await work();
      } finally {
        await client.query("SELECT pg_advisory_unlock($1)", [MIGRATION_LOCK_ID + 1]);
      }
      return true;
    } finally {
      client.release();
    }
  }

  async upsertEmailSuppression(input: {
    email: string;
    reason: "hard_bounce" | "complaint";
    eventId: string;
    messageId?: string;
    occurredAt: Date;
  }): Promise<void> {
    await this.#queryable().query(
      `INSERT INTO email_suppressions
        (email, reason, ses_event_id, ses_message_id, occurred_at)
       VALUES ($1, $2, $3, $4, $5)
       ON CONFLICT (email) DO UPDATE SET
         reason = EXCLUDED.reason,
         ses_event_id = EXCLUDED.ses_event_id,
         ses_message_id = EXCLUDED.ses_message_id,
         occurred_at = EXCLUDED.occurred_at,
         updated_at = now()
       WHERE email_suppressions.ses_event_id <> EXCLUDED.ses_event_id`,
      [input.email, input.reason, input.eventId, input.messageId ?? null, input.occurredAt],
    );
  }
}

async function runMigrations(connectionString: string): Promise<void> {
  const client = new Client({
    connectionString,
    application_name: "captures-migrator",
    connectionTimeoutMillis: 15_000,
  });
  await client.connect();
  try {
    await client.query("SELECT pg_advisory_lock($1)", [MIGRATION_LOCK_ID]);
    for (const migration of SHARING_MIGRATIONS) {
      await client.query("BEGIN");
      try {
        await client.query(migration.sql);
        await client.query("COMMIT");
      } catch (error) {
        await client.query("ROLLBACK");
        throw error;
      }
    }
  } finally {
    await client.query("SELECT pg_advisory_unlock($1)", [MIGRATION_LOCK_ID]).catch(() => {});
    await client.end();
  }
}
