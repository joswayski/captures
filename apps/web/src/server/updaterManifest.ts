export const PREVIEW_LATEST_JSON_URL =
  "https://github.com/joswayski/captures/releases/download/preview/latest.json";

/** How long the origin reuses a successful GitHub `latest.json` fetch. */
export const UPDATER_MANIFEST_CACHE_MS = 60_000;
/** How long to keep serving a stale copy after GitHub fails. */
export const UPDATER_MANIFEST_FAILURE_RETRY_MS = 60_000;
const GITHUB_TIMEOUT_MS = 5_000;
const MAX_MANIFEST_BYTES = 256 * 1024;

type Fetcher = (
  input: string | URL | Request,
  init?: RequestInit,
) => Promise<Response>;

type CacheEntry = {
  expiresAt: number;
  fetchedAt: number;
  text: string;
};

export type UpdaterManifestResult =
  | { ok: true; text: string; ageSeconds: number; stale: boolean }
  | { ok: false; error: string };

let cache: CacheEntry | null = null;
let inflight: Promise<string> | null = null;

export function resetUpdaterManifestCache(): void {
  cache = null;
  inflight = null;
}

/** Test helper: install a cached manifest with an explicit expiry. */
export function setUpdaterManifestCacheForTests(
  text: string,
  options: { expiresAt: number; fetchedAt?: number },
): void {
  cache = {
    text,
    expiresAt: options.expiresAt,
    fetchedAt: options.fetchedAt ?? Date.now(),
  };
  inflight = null;
}

export function isUpdaterManifest(value: unknown): boolean {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  if (typeof record.version !== "string" || !record.version.trim()) return false;
  return (
    Boolean(record.platforms) &&
    typeof record.platforms === "object" &&
    !Array.isArray(record.platforms)
  );
}

/**
 * Return GitHub's Preview `latest.json`, reusing the last successful lookup
 * so installed apps can poll captur.es without each hitting GitHub.
 */
export async function resolveUpdaterManifest(
  fetcher: Fetcher,
  now = Date.now(),
): Promise<UpdaterManifestResult> {
  if (cache && now < cache.expiresAt) {
    return {
      ok: true,
      text: cache.text,
      ageSeconds: ageSeconds(now, cache.fetchedAt),
      stale: false,
    };
  }

  if (inflight) {
    try {
      const text = await inflight;
      return {
        ok: true,
        text,
        ageSeconds: 0,
        stale: false,
      };
    } catch {
      return staleOrUnavailable(now);
    }
  }

  const promise = loadUpdaterManifest(fetcher);
  inflight = promise;
  try {
    const text = await promise;
    const fetchedAt = Date.now();
    cache = {
      text,
      fetchedAt,
      expiresAt: fetchedAt + UPDATER_MANIFEST_CACHE_MS,
    };
    return { ok: true, text, ageSeconds: 0, stale: false };
  } catch (error) {
    console.error("updater manifest lookup failed", error);
    if (cache) {
      cache = {
        ...cache,
        expiresAt: Date.now() + UPDATER_MANIFEST_FAILURE_RETRY_MS,
      };
      return {
        ok: true,
        text: cache.text,
        ageSeconds: ageSeconds(now, cache.fetchedAt),
        stale: true,
      };
    }
    return { ok: false, error: "updater manifest is unavailable" };
  } finally {
    if (inflight === promise) inflight = null;
  }
}

async function loadUpdaterManifest(fetcher: Fetcher): Promise<string> {
  const response = await fetcher(PREVIEW_LATEST_JSON_URL, {
    headers: {
      Accept: "application/json",
      "User-Agent": "captures-web",
    },
    signal: AbortSignal.timeout(GITHUB_TIMEOUT_MS),
  });
  if (!response.ok) {
    throw new Error(`GitHub request failed (${response.status})`);
  }

  const buffer = await response.arrayBuffer();
  if (buffer.byteLength > MAX_MANIFEST_BYTES) {
    throw new Error("updater manifest is too large");
  }

  const text = new TextDecoder().decode(buffer);
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error("updater manifest is not valid JSON");
  }
  if (!isUpdaterManifest(parsed)) {
    throw new Error("updater manifest is missing version or platforms");
  }
  return text;
}

function staleOrUnavailable(now: number): UpdaterManifestResult {
  if (!cache) return { ok: false, error: "updater manifest is unavailable" };
  return {
    ok: true,
    text: cache.text,
    ageSeconds: ageSeconds(now, cache.fetchedAt),
    stale: true,
  };
}

function ageSeconds(now: number, fetchedAt: number): number {
  return Math.max(0, Math.floor((now - fetchedAt) / 1000));
}
