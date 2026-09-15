const REPOSITORY = "joswayski/captures";
const ASSETS = new Set([
  "Captures-macOS-Apple-Silicon.dmg",
  "Captures-Windows-x64-setup.exe",
  "Captures-Linux-x64.deb",
  "Captures-Linux-x64.tar.gz",
]);
const CACHE_MS = 5 * 60 * 1_000;
let cachedTag: string | null = null;
let expiresAt = 0;
let inflight: Promise<void> | null = null;

async function refreshChannel() {
  try {
    const response = await fetch(`https://api.github.com/repos/${REPOSITORY}/releases/tags/native-preview`, {
      headers: { Accept: "application/vnd.github+json", "User-Agent": "captures-web" },
      signal: AbortSignal.timeout(5_000),
    });
    if (!response.ok) throw new Error("Native channel unavailable");
    const channel = await response.json() as { draft?: boolean; prerelease?: boolean; body?: string };
    const tag = typeof channel.body === "string"
      ? /^<!-- native-preview-tag: (native-v\d{4}\.\d{2}\.\d{2}\.[1-9]\d?) -->$/mu.exec(channel.body)?.[1]
      : null;
    if (channel.draft !== false || channel.prerelease !== true || !tag) {
      throw new Error("Native channel has no published pointer");
    }
    cachedTag = tag;
    expiresAt = Date.now() + CACHE_MS;
  } catch {
    // A previously verified immutable release is still safe during an outage.
    expiresAt = Date.now() + 60_000;
  }
}

/** Resolve the complete published set, never GitHub's mutable asset aliases. */
export async function nativeDownload(asset: string): Promise<Response> {
  if (!ASSETS.has(asset)) return new Response("Unknown download", { status: 404 });
  if (Date.now() >= expiresAt) {
    if (!inflight) inflight = refreshChannel().finally(() => { inflight = null; });
    await inflight;
  }
  if (!cachedTag) {
    return new Response("The native Preview is not available yet. Please try again shortly.", {
      status: 503,
      headers: { "Cache-Control": "no-store", "Retry-After": "60" },
    });
  }
  return new Response(null, {
    status: 302,
    headers: {
      Location: `https://github.com/${REPOSITORY}/releases/download/${cachedTag}/${asset}`,
      "Cache-Control": "public, max-age=60",
    },
  });
}
