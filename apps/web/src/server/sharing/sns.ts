import { createVerify } from "node:crypto";
import { normalizeEmail } from "./config.ts";

export interface SnsEnvelope {
  Type: "Notification" | "SubscriptionConfirmation" | "UnsubscribeConfirmation";
  MessageId: string;
  TopicArn: string;
  Message: string;
  Timestamp: string;
  SignatureVersion: "1" | "2";
  Signature: string;
  SigningCertURL: string;
  Subject?: string;
  Token?: string;
  SubscribeURL?: string;
}

export interface SuppressionEvent {
  email: string;
  reason: "hard_bounce" | "complaint";
  eventId: string;
  messageId?: string;
  occurredAt: Date;
}

const certificateCache = new Map<string, { value: string; expiresAt: number }>();

export async function verifySnsEnvelope(
  value: unknown,
  expectedTopicArn: string,
  fetcher: typeof fetch = fetch,
): Promise<SnsEnvelope> {
  const envelope = parseSnsEnvelope(value);
  if (!envelope || envelope.TopicArn !== expectedTopicArn) {
    throw new Error("unexpected SNS message");
  }
  const certificateUrl = validSnsUrl(envelope.SigningCertURL);
  const certificate = await loadCertificate(certificateUrl, fetcher);
  const verifier = createVerify(
    envelope.SignatureVersion === "2" ? "RSA-SHA256" : "RSA-SHA1",
  );
  verifier.update(canonicalSnsMessage(envelope), "utf8");
  verifier.end();
  if (!verifier.verify(certificate, envelope.Signature, "base64")) {
    throw new Error("invalid SNS signature");
  }
  return envelope;
}

export function extractSuppressionEvents(envelope: SnsEnvelope): SuppressionEvent[] {
  if (envelope.Type !== "Notification") return [];
  let message: unknown;
  try {
    message = JSON.parse(envelope.Message);
  } catch {
    return [];
  }
  if (!message || typeof message !== "object" || Array.isArray(message)) return [];
  const event = message as Record<string, unknown>;
  const mail = record(event.mail);
  const messageId = string(mail?.messageId);
  const eventType = event.eventType ?? event.notificationType;

  if (eventType === "Bounce") {
    const bounce = record(event.bounce);
    if (bounce?.bounceType !== "Permanent") return [];
    const occurredAt = validDate(
      string(bounce.timestamp) ?? string(mail?.timestamp) ?? envelope.Timestamp,
    );
    return recipients(bounce.bouncedRecipients).map((email) => ({
      email,
      reason: "hard_bounce",
      eventId: `${envelope.MessageId}:${email}`,
      messageId,
      occurredAt,
    }));
  }
  if (eventType === "Complaint") {
    const complaint = record(event.complaint);
    const occurredAt = validDate(
      string(complaint?.timestamp) ?? string(mail?.timestamp) ?? envelope.Timestamp,
    );
    return recipients(complaint?.complainedRecipients).map((email) => ({
      email,
      reason: "complaint",
      eventId: `${envelope.MessageId}:${email}`,
      messageId,
      occurredAt,
    }));
  }
  return [];
}

export async function confirmSnsSubscription(
  envelope: SnsEnvelope,
  fetcher: typeof fetch = fetch,
): Promise<void> {
  if (envelope.Type !== "SubscriptionConfirmation" || !envelope.SubscribeURL) return;
  const url = validSnsUrl(envelope.SubscribeURL);
  const response = await fetcher(url, {
    redirect: "error",
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) throw new Error(`SNS subscription confirmation failed: ${response.status}`);
}

function parseSnsEnvelope(value: unknown): SnsEnvelope | null {
  const input = record(value);
  if (!input) return null;
  const type = input.Type;
  const signatureVersion = input.SignatureVersion;
  if (
    type !== "Notification" &&
    type !== "SubscriptionConfirmation" &&
    type !== "UnsubscribeConfirmation"
  ) {
    return null;
  }
  if (signatureVersion !== "1" && signatureVersion !== "2") return null;
  for (const key of [
    "MessageId",
    "TopicArn",
    "Message",
    "Timestamp",
    "Signature",
    "SigningCertURL",
  ]) {
    if (typeof input[key] !== "string" || !input[key]) return null;
  }
  return input as unknown as SnsEnvelope;
}

function canonicalSnsMessage(envelope: SnsEnvelope): string {
  const fields: Array<keyof SnsEnvelope> = envelope.Type === "Notification"
    ? ["Message", "MessageId", ...(envelope.Subject ? ["Subject" as const] : []), "Timestamp", "TopicArn", "Type"]
    : ["Message", "MessageId", "SubscribeURL", "Timestamp", "Token", "TopicArn", "Type"];
  return fields.map((field) => `${field}\n${envelope[field] ?? ""}\n`).join("");
}

function validSnsUrl(value: string): URL {
  const url = new URL(value);
  const hostAllowed =
    url.hostname === "sns.amazonaws.com" ||
    /^sns\.[a-z0-9-]+\.amazonaws\.com(?:\.cn)?$/u.test(url.hostname);
  if (
    url.protocol !== "https:" ||
    !hostAllowed ||
    (url.port && url.port !== "443") ||
    url.username ||
    url.password
  ) {
    throw new Error("invalid SNS URL");
  }
  return url;
}

async function loadCertificate(url: URL, fetcher: typeof fetch): Promise<string> {
  if (!/^\/SimpleNotificationService-[A-Za-z0-9_-]+\.pem$/u.test(url.pathname)) {
    throw new Error("invalid SNS certificate path");
  }
  const cached = certificateCache.get(url.href);
  if (cached && cached.expiresAt > Date.now()) return cached.value;
  const response = await fetcher(url, {
    redirect: "error",
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) throw new Error(`SNS certificate request failed: ${response.status}`);
  const value = await response.text();
  if (!value.includes("BEGIN CERTIFICATE") || value.length > 16_384) {
    throw new Error("invalid SNS certificate");
  }
  certificateCache.set(url.href, { value, expiresAt: Date.now() + 60 * 60 * 1_000 });
  return value;
}

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function string(value: unknown): string | undefined {
  return typeof value === "string" && value ? value : undefined;
}

function recipients(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value
    .map((item) => normalizeEmail(string(record(item)?.emailAddress) ?? ""))
    .filter((email) => email.includes("@"));
}

function validDate(value: string): Date {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? new Date() : date;
}
