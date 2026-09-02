import { clientKeyFromRequest, type ApiEnv } from "./env.ts";
import {
  UPDATER_MANIFEST_CACHE_MS,
  resolveUpdaterManifest,
} from "./updaterManifest.ts";

const MAX_MESSAGE_LEN = 8_000;
const MAX_CONTACT_LEN = 200;
const MAX_META_LEN = 128;
const MAX_BODY_BYTES = 32 * 1024;
const DISCORD_DESCRIPTION_MAX = 4_000;
const DISCORD_TIMEOUT_MS = 10_000;

const ALLOWED_WEB_ORIGINS = new Set([
  "https://captur.es",
  "http://localhost:5174",
  "http://127.0.0.1:5174",
]);

export type { ApiEnv };

interface FeedbackInput {
  message: string;
  contact?: string;
  category: "bug" | "idea" | "other" | "crash";
  appVersion?: string;
  os?: string;
  osVersion?: string;
  arch?: string;
  source: "desktop" | "web";
}

interface DiscordField {
  name: string;
  value: string;
  inline: boolean;
}

interface DiscordWebhookPayload {
  embeds: Array<{
    title: string;
    description: string;
    color: number;
    fields: DiscordField[];
  }>;
}

type Fetcher = (
  input: string | URL | Request,
  init?: RequestInit,
) => Promise<Response>;

type ParseResult<T> =
  | { ok: true; value: T }
  | { ok: false; error: string };

export function getHealth(request: Request): Response {
  return json(request, { status: "ok" });
}

export async function getPreviewUpdaterManifest(
  request: Request,
  fetcher: Fetcher = fetch,
): Promise<Response> {
  const result = await resolveUpdaterManifest(fetcher);
  if (!result.ok) {
    return json(request, { error: result.error }, 502);
  }

  const headers = new Headers();
  headers.set("Content-Type", "application/json; charset=utf-8");
  headers.set(
    "Cache-Control",
    `public, max-age=${Math.floor(UPDATER_MANIFEST_CACHE_MS / 1000)}`,
  );
  headers.set("Age", String(result.ageSeconds));
  return new Response(result.text, { status: 200, headers });
}

export async function createFeedback(
  request: Request,
  env: ApiEnv,
  fetcher: Fetcher = fetch,
): Promise<Response> {
  const origin = request.headers.get("Origin");
  if (origin && !ALLOWED_WEB_ORIGINS.has(origin)) {
    return json(request, { error: "origin not allowed" }, 403);
  }

  const rawBody = await readBody(request);
  if (!rawBody.ok) {
    return json(request, { error: "request body is too large" }, 413);
  }

  let body: unknown;
  try {
    body = JSON.parse(rawBody.value);
  } catch {
    return json(request, { error: "request body must be valid JSON" }, 400);
  }

  const parsed = parseFeedback(body);
  if (!parsed.ok) {
    return json(request, { error: parsed.error }, 400);
  }

  const webhookUrl = env.DISCORD_WEBHOOK_URL;
  if (!webhookUrl || !isDiscordWebhookUrl(webhookUrl)) {
    console.error("DISCORD_WEBHOOK_URL is missing or invalid");
    return json(request, { error: "feedback service is not configured" }, 503);
  }

  const clientKey = clientKeyFromRequest(request);
  try {
    const { success } = await env.FEEDBACK_RATE_LIMITER.limit({
      key: `feedback:${clientKey}`,
    });
    if (!success) {
      return json(
        request,
        { error: "please wait a minute before sending more feedback" },
        429,
      );
    }
  } catch (error) {
    console.error("feedback rate limiter failed", error);
    return json(request, { error: "feedback service is temporarily unavailable" }, 503);
  }

  const discordPayload = buildDiscordPayload(parsed.value, clientKey);
  let response: Response;
  try {
    response = await fetcher(webhookUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(discordPayload),
      // Bound the outbound request so a stalled Discord connection cannot
      // pin this handler (and its socket) open indefinitely.
      signal: AbortSignal.timeout(DISCORD_TIMEOUT_MS),
    });
  } catch (error) {
    console.error("Discord webhook request failed", error);
    await refundRateLimit(env, clientKey);
    return json(request, { error: "failed to deliver feedback" }, 502);
  }

  if (!response.ok) {
    console.warn("Discord webhook rejected feedback", response.status);
    await refundRateLimit(env, clientKey);
    return json(request, { error: "failed to deliver feedback" }, 502);
  }

  return json(request, { ok: true }, 201);
}

async function refundRateLimit(env: ApiEnv, clientKey: string): Promise<void> {
  try {
    await env.FEEDBACK_RATE_LIMITER.refund?.({ key: `feedback:${clientKey}` });
  } catch (error) {
    console.error("feedback rate limiter refund failed", error);
  }
}

async function readBody(request: Request): Promise<ParseResult<string>> {
  const declaredLength = Number(request.headers.get("Content-Length"));
  if (Number.isFinite(declaredLength) && declaredLength > MAX_BODY_BYTES) {
    return { ok: false, error: "request body is too large" };
  }

  if (!request.body) return { ok: true, value: "" };

  const reader = request.body.getReader();
  const decoder = new TextDecoder();
  let bytesRead = 0;
  let value = "";

  while (true) {
    const chunk = await reader.read();
    if (chunk.done) break;
    bytesRead += chunk.value.byteLength;
    if (bytesRead > MAX_BODY_BYTES) {
      await reader.cancel();
      return { ok: false, error: "request body is too large" };
    }
    value += decoder.decode(chunk.value, { stream: true });
  }

  value += decoder.decode();
  return { ok: true, value };
}

function parseFeedback(value: unknown): ParseResult<FeedbackInput> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { ok: false, error: "request body must be a JSON object" };
  }

  const input = value as Record<string, unknown>;
  const message = normalizeRequired(input.message, "message", MAX_MESSAGE_LEN);
  if (!message.ok) return message;

  const contact = normalizeOptional(input.contact, "contact", MAX_CONTACT_LEN);
  if (!contact.ok) return contact;
  const appVersion = normalizeOptional(input.app_version, "app_version", MAX_META_LEN);
  if (!appVersion.ok) return appVersion;
  const os = normalizeOptional(input.os, "os", MAX_META_LEN);
  if (!os.ok) return os;
  const osVersion = normalizeOptional(input.os_version, "os_version", MAX_META_LEN);
  if (!osVersion.ok) return osVersion;
  const arch = normalizeOptional(input.arch, "arch", MAX_META_LEN);
  if (!arch.ok) return arch;

  const category = normalizeChoice(
    input.category ?? "bug",
    "category",
    ["bug", "idea", "other", "crash"] as const,
  );
  if (!category.ok) return category;
  const source = normalizeChoice(
    input.source ?? "desktop",
    "source",
    ["desktop", "web"] as const,
  );
  if (!source.ok) return source;

  return {
    ok: true,
    value: {
      message: message.value,
      contact: contact.value,
      category: category.value,
      appVersion: appVersion.value,
      os: os.value,
      osVersion: osVersion.value,
      arch: arch.value,
      source: source.value,
    },
  };
}

function normalizeRequired(
  value: unknown,
  field: string,
  maxLength: number,
): ParseResult<string> {
  if (typeof value !== "string" || !value.trim()) {
    return { ok: false, error: `${field} is required` };
  }
  const normalized = value.trim();
  if (Array.from(normalized).length > maxLength) {
    return { ok: false, error: `${field} must be at most ${maxLength} characters` };
  }
  return { ok: true, value: normalized };
}

function normalizeOptional(
  value: unknown,
  field: string,
  maxLength: number,
): ParseResult<string | undefined> {
  if (value === undefined || value === null || value === "") {
    return { ok: true, value: undefined };
  }
  if (typeof value !== "string") {
    return { ok: false, error: `${field} must be a string` };
  }
  const normalized = value.trim();
  if (!normalized) return { ok: true, value: undefined };
  if (Array.from(normalized).length > maxLength) {
    return { ok: false, error: `${field} must be at most ${maxLength} characters` };
  }
  return { ok: true, value: normalized };
}

function normalizeChoice<const T extends readonly string[]>(
  value: unknown,
  field: string,
  choices: T,
): ParseResult<T[number]> {
  if (typeof value !== "string") {
    return { ok: false, error: `${field} must be one of: ${choices.join(", ")}` };
  }
  const normalized = value.trim().toLowerCase();
  if (!choices.includes(normalized)) {
    return { ok: false, error: `${field} must be one of: ${choices.join(", ")}` };
  }
  return { ok: true, value: normalized as T[number] };
}

export function buildDiscordPayload(
  feedback: FeedbackInput,
  clientKey: string,
): DiscordWebhookPayload {
  const title =
    feedback.category === "idea"
      ? "Idea"
      : feedback.category === "other"
        ? "Other feedback"
        : feedback.category === "crash"
          ? "Crash report"
          : "Bug report";
  const color =
    feedback.category === "idea"
      ? 0x58a6ff
      : feedback.category === "other"
        ? 0x8b949e
        : feedback.category === "crash"
          ? 0xd29922
          : 0xef4650;

  const fields: DiscordField[] = [
    { name: "Category", value: feedback.category, inline: true },
    { name: "Source", value: feedback.source, inline: true },
  ];
  if (feedback.appVersion) {
    fields.push({ name: "App", value: feedback.appVersion, inline: true });
  }

  const system = [feedback.os, feedback.osVersion, feedback.arch]
    .filter((part): part is string => Boolean(part))
    .join(" · ");
  if (system) fields.push({ name: "System", value: system, inline: true });
  if (feedback.contact) {
    fields.push({ name: "Contact", value: feedback.contact, inline: true });
  }
  fields.push({ name: "Client", value: truncate(clientKey, 64), inline: true });

  return {
    embeds: [
      {
        title,
        description: formatDiscordDescription(feedback),
        color,
        fields,
      },
    ],
  };
}

function formatDiscordDescription(feedback: FeedbackInput): string {
  const message = feedback.message.trim();
  if (feedback.category !== "crash") {
    return truncate(message, DISCORD_DESCRIPTION_MAX);
  }

  const split = message.indexOf("\n\n");
  const intro = (split === -1 ? message : message.slice(0, split)).trim();
  const detail = split === -1 ? "" : message.slice(split).trim();
  if (!detail) {
    return truncate(intro, DISCORD_DESCRIPTION_MAX);
  }

  const fenceOpen = "\n\n```\n";
  const fenceClose = "\n```";
  const wrapperLength = fenceOpen.length + fenceClose.length;
  const boundedIntro = truncate(
    intro,
    Math.max(0, DISCORD_DESCRIPTION_MAX - wrapperLength),
  );
  const detailBudget = Math.max(
    0,
    DISCORD_DESCRIPTION_MAX - Array.from(boundedIntro).length - wrapperLength,
  );
  if (detailBudget === 0) {
    return boundedIntro;
  }
  const safeDetail = detail.replaceAll("```", "'''");
  return `${boundedIntro}${fenceOpen}${truncate(safeDetail, detailBudget)}${fenceClose}`;
}

function truncate(value: string, maxCharacters: number): string {
  const characters = Array.from(value);
  if (characters.length <= maxCharacters) return value;
  return `${characters.slice(0, Math.max(0, maxCharacters - 1)).join("")}…`;
}

function isDiscordWebhookUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      url.protocol === "https:" &&
      (url.hostname === "discord.com" || url.hostname === "discordapp.com") &&
      url.pathname.startsWith("/api/webhooks/")
    );
  } catch {
    return false;
  }
}

export function preflightFeedback(request: Request): Response {
  const origin = request.headers.get("Origin");
  if (origin && !ALLOWED_WEB_ORIGINS.has(origin)) {
    return json(request, { error: "origin not allowed" }, 403);
  }
  const headers = corsHeaders(request);
  headers.set("Access-Control-Allow-Headers", "Content-Type");
  headers.set("Access-Control-Allow-Methods", "POST, OPTIONS");
  headers.set("Access-Control-Max-Age", "86400");
  return new Response(null, { status: 204, headers });
}

function json(request: Request, body: unknown, status = 200): Response {
  const headers = corsHeaders(request);
  headers.set("Content-Type", "application/json; charset=utf-8");
  headers.set("Cache-Control", "no-store");
  return new Response(JSON.stringify(body), { status, headers });
}

function corsHeaders(request: Request): Headers {
  const headers = new Headers();
  const origin = request.headers.get("Origin");
  if (origin && ALLOWED_WEB_ORIGINS.has(origin)) {
    headers.set("Access-Control-Allow-Origin", origin);
    headers.set("Vary", "Origin");
  }
  return headers;
}
