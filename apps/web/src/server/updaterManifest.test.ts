import assert from "node:assert/strict";
import test from "node:test";

import {
  PREVIEW_LATEST_JSON_URL,
  isUpdaterManifest,
  resetUpdaterManifestCache,
  resolveUpdaterManifest,
  setUpdaterManifestCacheForTests,
} from "./updaterManifest.ts";

const MANIFEST = {
  version: "2026.9.201",
  notes: "Faster update checks.",
  platforms: {
    "darwin-aarch64": {
      url: "https://github.com/joswayski/captures/releases/download/v2026.09.02.1/Captures.app.tar.gz",
      signature: "signed",
    },
  },
};

function manifestText(overrides: Record<string, unknown> = {}) {
  return JSON.stringify({ ...MANIFEST, ...overrides });
}

test("isUpdaterManifest requires a version and platforms map", () => {
  assert.equal(isUpdaterManifest(MANIFEST), true);
  assert.equal(isUpdaterManifest({ version: "1", platforms: {} }), true);
  assert.equal(isUpdaterManifest({ version: "1" }), false);
  assert.equal(isUpdaterManifest({ platforms: {} }), false);
  assert.equal(isUpdaterManifest({ version: "1", platforms: [] }), false);
  assert.equal(isUpdaterManifest("nope"), false);
});

test("resolveUpdaterManifest fetches GitHub once and reuses the cache", async () => {
  resetUpdaterManifestCache();
  const urls: string[] = [];
  const fetcher = async (input: string | URL | Request) => {
    urls.push(String(input));
    return new Response(manifestText(), { status: 200 });
  };

  const first = await resolveUpdaterManifest(fetcher, 1_000);
  const second = await resolveUpdaterManifest(fetcher, 30_000);

  assert.equal(first.ok, true);
  assert.equal(second.ok, true);
  if (!first.ok || !second.ok) return;
  assert.equal(first.text, manifestText());
  assert.equal(second.text, first.text);
  assert.equal(second.stale, false);
  assert.deepEqual(urls, [PREVIEW_LATEST_JSON_URL]);
});

test("resolveUpdaterManifest refetches after the cache expires", async () => {
  resetUpdaterManifestCache();
  let fetches = 0;
  const fetcher = async () => {
    fetches += 1;
    return new Response(manifestText({ version: `2026.9.${fetches}` }), {
      status: 200,
    });
  };

  const first = await resolveUpdaterManifest(fetcher);
  assert.equal(first.ok, true);
  if (!first.ok) return;
  setUpdaterManifestCacheForTests(first.text, { expiresAt: 0, fetchedAt: 0 });

  const second = await resolveUpdaterManifest(fetcher, 70_000);
  assert.equal(second.ok, true);
  if (!second.ok) return;
  assert.equal(JSON.parse(second.text).version, "2026.9.2");
  assert.equal(fetches, 2);
});

test("resolveUpdaterManifest serves a stale copy when GitHub fails", async () => {
  resetUpdaterManifestCache();
  setUpdaterManifestCacheForTests(manifestText(), {
    expiresAt: 0,
    fetchedAt: 1_000,
  });

  const result = await resolveUpdaterManifest(async () => {
    throw new Error("network down");
  }, 80_000);

  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.equal(result.stale, true);
  assert.equal(result.text, manifestText());
  assert.equal(result.ageSeconds, 79);
});

test("resolveUpdaterManifest fails closed without a cached copy", async () => {
  resetUpdaterManifestCache();
  const result = await resolveUpdaterManifest(async () => {
    return new Response("missing", { status: 404 });
  });

  assert.deepEqual(result, {
    ok: false,
    error: "updater manifest is unavailable",
  });
});

test("resolveUpdaterManifest rejects a payload that is not an updater manifest", async () => {
  resetUpdaterManifestCache();
  const result = await resolveUpdaterManifest(async () => {
    return new Response(JSON.stringify({ hello: "world" }), { status: 200 });
  });

  assert.deepEqual(result, {
    ok: false,
    error: "updater manifest is unavailable",
  });
});

test("resolveUpdaterManifest rejects an oversized GitHub payload", async () => {
  resetUpdaterManifestCache();
  const huge = `${"a".repeat(256 * 1024 + 1)}`;
  const result = await resolveUpdaterManifest(async () => {
    return new Response(huge, { status: 200 });
  });

  assert.deepEqual(result, {
    ok: false,
    error: "updater manifest is unavailable",
  });
});

test("resolveUpdaterManifest coalesces concurrent GitHub lookups", async () => {
  resetUpdaterManifestCache();
  let fetches = 0;
  let release!: (response: Response) => void;
  const pending = new Promise<Response>((resolve) => {
    release = resolve;
  });
  const fetcher = async () => {
    fetches += 1;
    return pending;
  };

  const first = resolveUpdaterManifest(fetcher);
  const second = resolveUpdaterManifest(fetcher);
  release(new Response(manifestText(), { status: 200 }));

  const [a, b] = await Promise.all([first, second]);
  assert.equal(fetches, 1);
  assert.equal(a.ok, true);
  assert.equal(b.ok, true);
  if (!a.ok || !b.ok) return;
  assert.equal(a.text, b.text);
});
