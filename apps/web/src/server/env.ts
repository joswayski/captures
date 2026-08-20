import { createMemoryRateLimiter, type RateLimiter } from "./rateLimit.ts";

export interface ApiEnv {
  DISCORD_WEBHOOK_URL?: string;
  FEEDBACK_RATE_LIMITER: RateLimiter;
}

const feedbackRateLimiter = createMemoryRateLimiter({
  limit: 1,
  periodMs: 60_000,
});

export function getApiEnv(): ApiEnv {
  return {
    DISCORD_WEBHOOK_URL: process.env.DISCORD_WEBHOOK_URL,
    FEEDBACK_RATE_LIMITER: feedbackRateLimiter,
  };
}

/** Prefer Cloudflare's connecting IP when the edge is in front of the origin. */
export function clientKeyFromRequest(request: Request): string {
  const cloudflareIp = request.headers.get("CF-Connecting-IP")?.trim();
  if (cloudflareIp) return cloudflareIp;

  const realIp = request.headers.get("X-Real-IP")?.trim();
  if (realIp) return realIp;

  return "unknown";
}
