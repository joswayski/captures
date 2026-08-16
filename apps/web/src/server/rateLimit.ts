export interface RateLimiter {
  limit(input: { key: string }): Promise<{ success: boolean }>;
}

export function createMemoryRateLimiter(options: {
  limit: number;
  periodMs: number;
}): RateLimiter {
  const hits = new Map<string, number[]>();

  return {
    async limit({ key }) {
      const now = Date.now();
      const windowStart = now - options.periodMs;
      const recent = (hits.get(key) ?? []).filter((timestamp) => timestamp > windowStart);

      if (recent.length >= options.limit) {
        hits.set(key, recent);
        return { success: false };
      }

      recent.push(now);
      hits.set(key, recent);
      return { success: true };
    },
  };
}
