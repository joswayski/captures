import { createMemoryRateLimiter, type RateLimiter } from "./rateLimit.ts";
import { readSharingConfig } from "./sharing/config.ts";

export interface SharingRequestHandler {
  handle(request: Request): Promise<Response | null>;
}

export interface ApiEnv {
  DISCORD_WEBHOOK_URL?: string;
  FEEDBACK_RATE_LIMITER: RateLimiter;
  SHARING_API?: SharingRequestHandler;
  SHARING_READY?: Promise<unknown>;
}

const feedbackRateLimiter = createMemoryRateLimiter({
  limit: 1,
  periodMs: 60_000,
});

const sharingConfig = readSharingConfig();
let sharingApiPromise: Promise<SharingRequestHandler> | undefined;

export function initializeSharingApi(): Promise<SharingRequestHandler> | undefined {
  if (!sharingConfig.enabled) return undefined;
  sharingApiPromise ??= import("./sharing/api.ts")
    .then(({ createSharingApi }) => createSharingApi(sharingConfig))
    .catch((error) => {
      console.error("Captures sharing failed to initialize", error);
      throw error;
    });
  return sharingApiPromise;
}

const sharingHandler: SharingRequestHandler | undefined = sharingConfig.enabled
  ? {
      async handle(request) {
        try {
          return await initializeSharingApi()!.then((api) => api.handle(request));
        } catch {
          return new Response(
            JSON.stringify({ error: "sharing service is temporarily unavailable" }),
            {
              status: 503,
              headers: {
                "Cache-Control": "no-store",
                "Content-Type": "application/json; charset=utf-8",
              },
            },
          );
        }
      },
    }
  : undefined;

void initializeSharingApi()?.catch(() => undefined);

export function getApiEnv(): ApiEnv {
  return {
    DISCORD_WEBHOOK_URL: process.env.DISCORD_WEBHOOK_URL,
    FEEDBACK_RATE_LIMITER: feedbackRateLimiter,
    SHARING_API: sharingHandler,
    SHARING_READY: sharingApiPromise,
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
