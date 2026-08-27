import assert from "node:assert/strict";
import test from "node:test";

import { createMemoryRateLimiter } from "./rateLimit.ts";

test("allows the first request in a window and rejects the next", async () => {
  const limiter = createMemoryRateLimiter({ limit: 1, periodMs: 60_000 });

  assert.deepEqual(await limiter.limit({ key: "feedback:203.0.113.9" }), {
    success: true,
  });
  assert.deepEqual(await limiter.limit({ key: "feedback:203.0.113.9" }), {
    success: false,
  });
});

test("tracks keys independently", async () => {
  const limiter = createMemoryRateLimiter({ limit: 1, periodMs: 60_000 });

  assert.deepEqual(await limiter.limit({ key: "feedback:a" }), { success: true });
  assert.deepEqual(await limiter.limit({ key: "feedback:b" }), { success: true });
  assert.deepEqual(await limiter.limit({ key: "feedback:a" }), { success: false });
});

test("refund restores a slot after a failed delivery", async () => {
  const limiter = createMemoryRateLimiter({ limit: 1, periodMs: 60_000 });

  assert.deepEqual(await limiter.limit({ key: "feedback:203.0.113.9" }), {
    success: true,
  });
  await limiter.refund?.({ key: "feedback:203.0.113.9" });
  assert.deepEqual(await limiter.limit({ key: "feedback:203.0.113.9" }), {
    success: true,
  });
});
