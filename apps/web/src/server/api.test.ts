import assert from "node:assert/strict";
import test from "node:test";

import { createMemoryRateLimiter } from "./rateLimit.ts";
import { buildDiscordPayload, handleApiRequest, type ApiEnv } from "./api.ts";

function createEnv(options: { rateLimitSuccess?: boolean; webhook?: string } = {}) {
  const rateLimitKeys: string[] = [];
  const env: ApiEnv = {
    DISCORD_WEBHOOK_URL:
      options.webhook ?? "https://discord.com/api/webhooks/123/example-token",
    FEEDBACK_RATE_LIMITER: {
      async limit({ key }: { key: string }) {
        rateLimitKeys.push(key);
        return { success: options.rateLimitSuccess ?? true };
      },
    },
  };
  return { env, rateLimitKeys };
}

test("serves health only from the API path", async () => {
  const { env } = createEnv();
  const response = await handleApiRequest(
    new Request("https://captur.es/api/health"),
    env,
  );

  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { status: "ok" });

  const missing = await handleApiRequest(
    new Request("https://captur.es/health"),
    env,
  );
  assert.equal(missing.status, 404);
});

test("validates feedback before rate limiting or calling Discord", async () => {
  const { env, rateLimitKeys } = createEnv();
  let fetchCalls = 0;
  const response = await handleApiRequest(
    new Request("https://captur.es/api/feedback", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ message: "   " }),
    }),
    env,
    async () => {
      fetchCalls += 1;
      return new Response(null, { status: 204 });
    },
  );

  assert.equal(response.status, 400);
  assert.deepEqual(await response.json(), { error: "message is required" });
  assert.deepEqual(rateLimitKeys, []);
  assert.equal(fetchCalls, 0);
});

test("rejects oversized feedback before rate limiting or calling Discord", async () => {
  const { env, rateLimitKeys } = createEnv();
  let fetchCalls = 0;
  const response = await handleApiRequest(
    new Request("https://captur.es/api/feedback", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "x".repeat(32 * 1024 + 1),
    }),
    env,
    async () => {
      fetchCalls += 1;
      return new Response(null, { status: 204 });
    },
  );

  assert.equal(response.status, 413);
  assert.deepEqual(await response.json(), {
    error: "request body is too large",
  });
  assert.deepEqual(rateLimitKeys, []);
  assert.equal(fetchCalls, 0);
});

test("rate limits accepted feedback by the Cloudflare connecting IP", async () => {
  const { env, rateLimitKeys } = createEnv({ rateLimitSuccess: false });
  let fetchCalls = 0;
  const response = await handleApiRequest(
    new Request("https://captur.es/api/feedback", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "CF-Connecting-IP": "203.0.113.9",
      },
      body: JSON.stringify({ message: "Recording freezes" }),
    }),
    env,
    async () => {
      fetchCalls += 1;
      return new Response(null, { status: 204 });
    },
  );

  assert.equal(response.status, 429);
  assert.deepEqual(rateLimitKeys, ["feedback:203.0.113.9"]);
  assert.equal(fetchCalls, 0);
});

test("ignores a client-supplied X-Forwarded-For address", async () => {
  const { env, rateLimitKeys } = createEnv();
  const response = await handleApiRequest(
    new Request("https://captur.es/api/feedback", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Forwarded-For": "198.51.100.10",
        "X-Real-IP": "203.0.113.11",
      },
      body: JSON.stringify({ message: "Recording freezes" }),
    }),
    env,
    async () => new Response(null, { status: 204 }),
  );

  assert.equal(response.status, 201);
  assert.deepEqual(rateLimitKeys, ["feedback:203.0.113.11"]);
});

test("delivers normalized feedback to Discord", async () => {
  const { env, rateLimitKeys } = createEnv();
  let webhookUrl = "";
  let webhookBody: unknown;
  const response = await handleApiRequest(
    new Request("https://captur.es/api/feedback", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "CF-Connecting-IP": "203.0.113.10",
        Origin: "https://captur.es",
      },
      body: JSON.stringify({
        message: "  Recording freezes  ",
        contact: "  @jose  ",
        category: "BUG",
        app_version: "0.1.0",
        os: "macos",
        os_version: "15.5",
        arch: "aarch64",
        source: "desktop",
      }),
    }),
    env,
    async (input, init) => {
      webhookUrl = String(input);
      webhookBody = JSON.parse(String(init?.body));
      return new Response(null, { status: 204 });
    },
  );

  assert.equal(response.status, 201);
  assert.deepEqual(await response.json(), { ok: true });
  assert.equal(response.headers.get("Access-Control-Allow-Origin"), "https://captur.es");
  assert.equal(webhookUrl, env.DISCORD_WEBHOOK_URL);
  assert.deepEqual(rateLimitKeys, ["feedback:203.0.113.10"]);
  assert.deepEqual(webhookBody, {
    embeds: [
      {
        title: "Bug report",
        description: "Recording freezes",
        color: 0xef4650,
        fields: [
          { name: "Category", value: "bug", inline: true },
          { name: "Source", value: "desktop", inline: true },
          { name: "App", value: "0.1.0", inline: true },
          { name: "System", value: "macos · 15.5 · aarch64", inline: true },
          { name: "Contact", value: "@jose", inline: true },
          { name: "Client", value: "203.0.113.10", inline: true },
        ],
      },
    ],
  });
});

test("delivers crash reports to Discord as a distinct category", async () => {
  const { env } = createEnv();
  let webhookBody: unknown;
  const response = await handleApiRequest(
    new Request("https://captur.es/api/feedback", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "CF-Connecting-IP": "203.0.113.12",
      },
      body: JSON.stringify({
        message: "Captures closed unexpectedly.\n\nException Type: EXC_BREAKPOINT (SIGTRAP)",
        category: "crash",
        app_version: "2026.8.2813",
        os: "macos",
        os_version: "26.6.1",
        arch: "aarch64",
        source: "desktop",
      }),
    }),
    env,
    async (_input, init) => {
      webhookBody = JSON.parse(String(init?.body));
      return new Response(null, { status: 204 });
    },
  );

  assert.equal(response.status, 201);
  const embed = (
    webhookBody as { embeds: Array<{ title: string; color: number; description: string }> }
  ).embeds[0];
  assert.equal(embed.title, "Crash report");
  assert.equal(embed.color, 0xd29922);
  assert.match(embed.description, /Exception Type: EXC_BREAKPOINT \(SIGTRAP\)/u);
  assert.match(embed.description, /```[\s\S]*EXC_BREAKPOINT/u);
});

test("puts crash exception details in a Discord code block", () => {
  const payload = buildDiscordPayload(
    {
      message:
        "Captures closed unexpectedly. This is an automatic crash diagnostic; it does not include captures.\n\nPanic:\nthread 'main' panicked at src/lib.rs:1:1:\nboom",
      category: "crash",
      source: "desktop",
      appVersion: "2026.8.2820",
      os: "windows",
      osVersion: "Windows_NT",
      arch: "x86_64",
    },
    "client",
  );

  assert.match(payload.embeds[0].description, /```\nPanic:/u);
  assert.match(payload.embeds[0].description, /thread 'main' panicked/u);
  assert.match(payload.embeds[0].description, /boom/u);
});

test("keeps Discord descriptions within the embed limit", () => {
  const payload = buildDiscordPayload(
    {
      message: "a".repeat(8_000),
      category: "idea",
      source: "web",
    },
    "client",
  );

  assert.equal(Array.from(payload.embeds[0].description).length, 4_000);
  assert.match(payload.embeds[0].description, /…$/u);
});

test("refunds the rate limit when Discord delivery fails", async () => {
  const env: ApiEnv = {
    DISCORD_WEBHOOK_URL: "https://discord.com/api/webhooks/123/example-token",
    FEEDBACK_RATE_LIMITER: createMemoryRateLimiter({ limit: 1, periodMs: 60_000 }),
  };
  const request = () =>
    new Request("https://captur.es/api/feedback", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "CF-Connecting-IP": "203.0.113.9",
      },
      body: JSON.stringify({ message: "Recording freezes" }),
    });

  const failed = await handleApiRequest(request(), env, async () => {
    throw new Error("network down");
  });
  assert.equal(failed.status, 502);

  const retry = await handleApiRequest(
    request(),
    env,
    async () => new Response(null, { status: 204 }),
  );
  assert.equal(retry.status, 201);
});
