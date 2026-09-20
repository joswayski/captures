import type { SharePageData, ShareMetadata } from "../shareModel";

export async function fetchShareMetadata(
  id: string,
  cookie: string | undefined,
  origin = process.env.CAPTURES_API_ORIGIN?.trim() || "http://127.0.0.1:3001",
): Promise<SharePageData> {
  if (
    typeof id !== "string" ||
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)
  ) {
    return { kind: "missing" };
  }
  try {
    const url = new URL(`/api/shares/${encodeURIComponent(id)}`, origin);
    const response = await fetch(url, {
      headers: cookie ? { cookie } : {},
      redirect: "error",
      cache: "no-store",
      signal: AbortSignal.timeout(5_000),
    });
    if (response.status === 404) return { kind: "missing" };
    if (!response.ok) return { kind: "unavailable" };
    return { kind: "ready", share: (await response.json()) as ShareMetadata };
  } catch {
    return { kind: "unavailable" };
  }
}
