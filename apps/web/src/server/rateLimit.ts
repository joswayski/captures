export interface RateLimiter {
  limit(input: { key: string }): Promise<{ success: boolean }>;
  refund?(input: { key: string }): Promise<void>;
}

export function createMemoryRateLimiter(options: {
  limit: number;
  periodMs: number;
}): RateLimiter {
  const hits = new Map<string, number[]>();
  let nextSweepAt = 0;

  // Drop keys whose hits have all aged out so the map cannot grow without
  // bound as distinct clients come and go over the process lifetime.
  const sweep = (windowStart: number) => {
    for (const [key, timestamps] of hits) {
      if (!timestamps.some((timestamp) => timestamp > windowStart)) {
        hits.delete(key);
      }
    }
  };

  return {
    async limit({ key }) {
      const now = Date.now();
      const windowStart = now - options.periodMs;
      if (now >= nextSweepAt) {
        nextSweepAt = now + options.periodMs;
        sweep(windowStart);
      }
      const recent = (hits.get(key) ?? []).filter((timestamp) => timestamp > windowStart);

      if (recent.length >= options.limit) {
        hits.set(key, recent);
        return { success: false };
      }

      recent.push(now);
      hits.set(key, recent);
      return { success: true };
    },
    async refund({ key }) {
      const now = Date.now();
      const windowStart = now - options.periodMs;
      const recent = (hits.get(key) ?? []).filter((timestamp) => timestamp > windowStart);
      recent.pop();
      if (recent.length === 0) {
        hits.delete(key);
      } else {
        hits.set(key, recent);
      }
    },
  };
}
