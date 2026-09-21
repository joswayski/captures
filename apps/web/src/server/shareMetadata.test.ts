import assert from "node:assert/strict";
import test from "node:test";
import { fetchShareMetadata } from "./shareMetadata.ts";

test("share lookup rejects paths before forwarding viewer cookies", async (t) => {
  const fetch = t.mock.method(globalThis, "fetch", async () => {
    throw new Error("must not fetch");
  });
  for (const id of [
    "..",
    "../account/me",
    "https://example.com",
    "",
    "invalid",
  ]) {
    assert.deepEqual(await fetchShareMetadata(id, "viewer=test"), {
      kind: "missing",
    });
  }
  assert.equal(fetch.mock.callCount(), 0);
});

test("share metadata goes only to configured API, without redirects or caching", async (t) => {
  const id = "Ab_cdEF012-3";
  t.mock.method(globalThis, "fetch", async (url: URL, init: RequestInit) => {
    assert.equal(url.href, `http://captures-api/api/shares/${id}`);
    assert.equal(init.redirect, "error");
    assert.equal(init.cache, "no-store");
    assert.deepEqual(init.headers, { cookie: "viewer=test" });
    return new Response(null, { status: 404 });
  });
  assert.deepEqual(
    await fetchShareMetadata(id, "viewer=test", "http://captures-api"),
    { kind: "missing" },
  );
});
