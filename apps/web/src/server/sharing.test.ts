import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { verifyLoginCodeAttempt } from "./sharing/auth.ts";
import { clientIpAllowed, readSharingConfig, validateSharingConfig } from "./sharing/config.ts";
import { clientIpHmac, loginCodeHmac, safeEqual } from "./sharing/crypto.ts";
import { extractSuppressionEvents, type SnsEnvelope } from "./sharing/sns.ts";
import { GIB } from "./sharing/types.ts";
import {
  expectedMimeMatches,
  parseCreateAsset,
  parseShareExpiry,
} from "./sharing/validation.ts";

const SHA256 = Buffer.alloc(32).toString("base64");

test("sharing migration creates exactly the five product tables", () => {
  const sql = readFileSync(
    new URL("../../migrations/0001_asset_sharing.sql", import.meta.url),
    "utf8",
  );
  assert.deepEqual(
    [...sql.matchAll(/CREATE TABLE IF NOT EXISTS ([a-z_]+)/gu)].map((match) => match[1]),
    ["users", "sessions", "login_codes", "assets", "email_suppressions"],
  );
  assert.equal(sql.includes("schema_migrations"), false);
});

test("private beta config requires allowlists and every external dependency", () => {
  const config = readSharingConfig({ SHARING_ENABLED: "true" });
  assert.deepEqual(validateSharingConfig(config), [
    "DATABASE_URL",
    "DATABASE_MIGRATION_URL",
    "STORAGE_ENDPOINT",
    "STORAGE_BUCKET",
    "STORAGE_ACCESS_KEY_ID",
    "STORAGE_SECRET_ACCESS_KEY",
    "AUTH_CODE_HMAC_KEY (base64, at least 32 bytes)",
    "AUTH_ALLOWED_EMAILS",
    "AUTH_ALLOWED_CIDRS",
    "SES_SMTP_HOST",
    "SES_SMTP_USERNAME",
    "SES_SMTP_PASSWORD",
    "SES_SNS_TOPIC_ARN",
  ]);
});

test("public signup fails closed until both clients implement Turnstile", () => {
  const config = readSharingConfig({
    SHARING_ENABLED: "true",
    DATABASE_URL: "postgresql://runtime",
    DATABASE_MIGRATION_URL: "postgresql://migration",
    STORAGE_ENDPOINT: "https://storage.example.com",
    STORAGE_BUCKET: "captures",
    STORAGE_ACCESS_KEY_ID: "access",
    STORAGE_SECRET_ACCESS_KEY: "secret",
    AUTH_CODE_HMAC_KEY: Buffer.alloc(32, 1).toString("base64"),
    AUTH_ALLOWED_EMAILS: "owner@example.com",
    AUTH_ALLOWED_CIDRS: "203.0.113.8/32",
    AUTH_PUBLIC_SIGNUP: "true",
    SES_SMTP_HOST: "email-smtp.us-east-1.amazonaws.com",
    SES_SMTP_USERNAME: "smtp-user",
    SES_SMTP_PASSWORD: "smtp-password",
    SES_SNS_TOPIC_ARN: "arn:aws:sns:us-east-1:123456789012:captures",
  });

  assert.deepEqual(validateSharingConfig(config), [
    "AUTH_PUBLIC_SIGNUP must remain false until web and desktop Turnstile flows are implemented",
  ]);
});

test("private beta CIDR gate handles IPv4, IPv6, and mapped IPv4", () => {
  const cidrs = ["203.0.113.8/32", "2001:db8::/32"];
  assert.equal(clientIpAllowed("203.0.113.8", cidrs), true);
  assert.equal(clientIpAllowed("::ffff:203.0.113.8", cidrs), true);
  assert.equal(clientIpAllowed("203.0.113.9", cidrs), false);
  assert.equal(clientIpAllowed("2001:db8::4", cidrs), true);
  assert.equal(clientIpAllowed("not-an-ip", cidrs), false);
});

test("asset reservation accepts the supported kinds and enforces the account object cap", () => {
  const parsed = parseCreateAsset({
    kind: "video",
    title: "Demo",
    original: { bytes: GIB - 1, mime_type: "video/mp4", sha256: SHA256 },
    preview: { bytes: 1, mime_type: "image/png", sha256: SHA256 },
    width: 1920,
    height: 1080,
    duration_ms: 3_000,
  });
  assert.equal(parsed.ok, true);

  const tooLarge = parseCreateAsset({
    kind: "screenshot",
    original: { bytes: GIB, mime_type: "image/png", sha256: SHA256 },
    preview: { bytes: 1, mime_type: "image/png", sha256: SHA256 },
  });
  assert.deepEqual(tooLarge, {
    ok: false,
    error: "original and preview together must not exceed 1 GiB",
  });
});

test("magic-byte validation rejects an extension-only claim", async () => {
  const pngPrefix = Buffer.from("89504e470d0a1a0a0000000d49484452", "hex");
  assert.equal(await expectedMimeMatches("screenshot", "image/png", pngPrefix), true);
  assert.equal(await expectedMimeMatches("screenshot", "image/jpeg", pngPrefix), false);
});

test("login codes and client IPs use purpose-separated HMACs", () => {
  const key = Buffer.alloc(32, 7);
  const code = loginCodeHmac(key, "owner@example.com", "123456");
  const same = loginCodeHmac(key, "owner@example.com", "123456");
  const ip = clientIpHmac(key, "203.0.113.8");
  assert.equal(safeEqual(code, same), true);
  assert.equal(safeEqual(code, ip), false);
});

test("an invalid login code commits its failed-attempt increment", async () => {
  const email = "owner@example.com";
  const hmacKey = Buffer.alloc(32, 7);
  let attempts = 0;
  let transactionOutcome = "";
  async function withTransaction<T>(work: (client: object) => Promise<T>): Promise<T> {
    try {
      const result = await work({});
      transactionOutcome = "commit";
      return result;
    } catch (error) {
      transactionOutcome = "rollback";
      throw error;
    }
  }
  const issued = await withTransaction((client) =>
    verifyLoginCodeAttempt(client, {
      async findLoginCode() {
        return {
          id: "login-code",
          codeHmac: loginCodeHmac(hmacKey, email, "654321"),
          attempts,
          expiresAt: new Date(Date.now() + 60_000),
        };
      },
      expectedCodeHmac: loginCodeHmac(hmacKey, email, "123456"),
      async recordFailedAttempt() {
        attempts += 1;
      },
      async consumeLoginCode() {
        assert.fail("an invalid code must not be consumed");
      },
      async onSuccess() {
        assert.fail("an invalid code must not issue a session");
      },
    }),
  );

  assert.equal(issued, null);
  assert.equal(attempts, 1);
  assert.equal(transactionOutcome, "commit");
});

test("share expiry rejects unsafe or unrepresentable durations", () => {
  assert.equal(
    parseShareExpiry({ access: "shared", expires_in_seconds: Number.MAX_SAFE_INTEGER }),
    undefined,
  );
  assert.equal(
    parseShareExpiry({ access: "shared", expires_in_seconds: Number.MAX_VALUE }),
    undefined,
  );
  assert.equal(parseShareExpiry({ access: "shared", expires_in_seconds: null }), null);
});

test("SES hard bounces and complaints become per-address suppression events", () => {
  const base: SnsEnvelope = {
    Type: "Notification",
    MessageId: "sns-event",
    TopicArn: "arn:aws:sns:us-east-1:123456789012:captures",
    Message: "",
    Timestamp: "2026-08-30T12:00:00.000Z",
    SignatureVersion: "1",
    Signature: "signature",
    SigningCertURL:
      "https://sns.us-east-1.amazonaws.com/SimpleNotificationService-example.pem",
  };
  const bounce = extractSuppressionEvents({
    ...base,
    Message: JSON.stringify({
      eventType: "Bounce",
      mail: { messageId: "ses-message", timestamp: base.Timestamp },
      bounce: {
        bounceType: "Permanent",
        timestamp: "2026-08-30T12:01:00.000Z",
        bouncedRecipients: [{ emailAddress: "Bad@Example.com" }],
      },
    }),
  });
  assert.deepEqual(bounce, [
    {
      email: "bad@example.com",
      reason: "hard_bounce",
      eventId: "sns-event:bad@example.com",
      messageId: "ses-message",
      occurredAt: new Date("2026-08-30T12:01:00.000Z"),
    },
  ]);

  const transient = extractSuppressionEvents({
    ...base,
    Message: JSON.stringify({
      notificationType: "Bounce",
      bounce: {
        bounceType: "Transient",
        bouncedRecipients: [{ emailAddress: "later@example.com" }],
      },
    }),
  });
  assert.deepEqual(transient, []);
});
