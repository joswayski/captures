import { isIP } from "node:net";
import ipaddr from "ipaddr.js";
import type { SharingConfig } from "./types.ts";

const DEFAULT_PUBLIC_ORIGIN = "https://captur.es";

function optional(value: string | undefined): string | undefined {
  const normalized = value?.trim();
  return normalized ? normalized : undefined;
}

function csv(value: string | undefined): string[] {
  return (value ?? "")
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function bool(value: string | undefined, fallback = false): boolean {
  if (value === undefined || value.trim() === "") return fallback;
  return value.trim().toLowerCase() === "true";
}

function integer(value: string | undefined, fallback: number): number {
  if (!value) return fallback;
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : fallback;
}

export function readSharingConfig(
  environment: NodeJS.ProcessEnv = process.env,
): SharingConfig {
  const publicOrigin = optional(environment.PUBLIC_ORIGIN) ?? DEFAULT_PUBLIC_ORIGIN;
  const codeHmac = optional(environment.AUTH_CODE_HMAC_KEY);

  return {
    enabled: bool(environment.SHARING_ENABLED),
    databaseUrl: optional(environment.DATABASE_URL),
    migrationDatabaseUrl:
      optional(environment.DATABASE_MIGRATION_URL) ?? optional(environment.DATABASE_URL),
    publicOrigin: new URL(publicOrigin).origin,
    storage: {
      backend: optional(environment.STORAGE_BACKEND) ?? "r2",
      endpoint: optional(environment.STORAGE_ENDPOINT),
      region: optional(environment.STORAGE_REGION) ?? "auto",
      bucket: optional(environment.STORAGE_BUCKET) ?? "",
      accessKeyId: optional(environment.STORAGE_ACCESS_KEY_ID),
      secretAccessKey: optional(environment.STORAGE_SECRET_ACCESS_KEY),
    },
    auth: {
      codeHmacKey: codeHmac ? Buffer.from(codeHmac, "base64") : undefined,
      allowedEmails: new Set(
        csv(environment.AUTH_ALLOWED_EMAILS).map((email) => normalizeEmail(email)),
      ),
      allowedCidrs: csv(environment.AUTH_ALLOWED_CIDRS),
      publicSignup: bool(environment.AUTH_PUBLIC_SIGNUP),
      googleClientId: optional(environment.GOOGLE_CLIENT_ID),
      googleClientSecret: optional(environment.GOOGLE_CLIENT_SECRET),
    },
    mail: {
      host: optional(environment.SES_SMTP_HOST),
      port: integer(environment.SES_SMTP_PORT, 587),
      secure: bool(environment.SES_SMTP_SECURE),
      user: optional(environment.SES_SMTP_USERNAME),
      password: optional(environment.SES_SMTP_PASSWORD),
      from: optional(environment.SES_FROM_EMAIL) ?? "Captures <login@captur.es>",
      configurationSet:
        optional(environment.SES_CONFIGURATION_SET) ?? "captures-auth",
      tenant: optional(environment.SES_TENANT) ?? "captures",
      snsTopicArn: optional(environment.SES_SNS_TOPIC_ARN),
    },
  };
}

export function validateSharingConfig(config: SharingConfig): string[] {
  if (!config.enabled) return [];
  const missing: string[] = [];
  if (!config.databaseUrl) missing.push("DATABASE_URL");
  if (!config.migrationDatabaseUrl) missing.push("DATABASE_MIGRATION_URL");
  if (!config.storage.endpoint) missing.push("STORAGE_ENDPOINT");
  if (!config.storage.bucket) missing.push("STORAGE_BUCKET");
  if (!config.storage.accessKeyId) missing.push("STORAGE_ACCESS_KEY_ID");
  if (!config.storage.secretAccessKey) missing.push("STORAGE_SECRET_ACCESS_KEY");
  if (!config.auth.codeHmacKey || config.auth.codeHmacKey.byteLength < 32) {
    missing.push("AUTH_CODE_HMAC_KEY (base64, at least 32 bytes)");
  }
  if (config.auth.publicSignup) {
    missing.push(
      "AUTH_PUBLIC_SIGNUP must remain false until web and desktop Turnstile flows are implemented",
    );
  }
  if (config.auth.allowedEmails.size === 0) missing.push("AUTH_ALLOWED_EMAILS");
  if (config.auth.allowedCidrs.length === 0) missing.push("AUTH_ALLOWED_CIDRS");
  if (!config.mail.host) missing.push("SES_SMTP_HOST");
  if (!config.mail.user) missing.push("SES_SMTP_USERNAME");
  if (!config.mail.password) missing.push("SES_SMTP_PASSWORD");
  if (!config.mail.snsTopicArn) missing.push("SES_SNS_TOPIC_ARN");
  return missing;
}

export function normalizeEmail(value: string): string {
  return value.trim().toLowerCase();
}

export function clientIpAllowed(ip: string, cidrs: string[]): boolean {
  if (isIP(ip) === 0) return false;
  let address = ipaddr.parse(ip);
  if (address instanceof ipaddr.IPv6 && address.isIPv4MappedAddress()) {
    address = address.toIPv4Address();
  }

  return cidrs.some((cidr) => {
    try {
      const [range, prefix] = ipaddr.parseCIDR(cidr);
      const normalizedRange = range instanceof ipaddr.IPv6 && range.isIPv4MappedAddress()
        ? range.toIPv4Address()
        : range;
      return address.kind() === normalizedRange.kind() && address.match(normalizedRange, prefix);
    } catch {
      return false;
    }
  });
}
