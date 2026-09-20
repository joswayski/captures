import assert from "node:assert/strict";
import test from "node:test";
import { handleRequest, type Env } from "../src/index.ts";

const id = "Ab_cdEF012-3";
const bytes = new TextEncoder().encode("0123456789");

function setup(api: (request: Request) => Response | Promise<Response> = () => Response.json({ key: `assets/${id}`, name: "shot.png", contentType: "image/png", byteSize: bytes.length })) {
  const originalFetch = globalThis.fetch;
  const calls: string[] = [];
  let gets = 0;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    assert.equal(init?.redirect, "manual");
    assert.equal(init?.cache, "no-store");
    assert.ok(init?.signal);
    const request = new Request(input, init);
    calls.push(`${request.headers.get("cookie")}|${request.headers.get("authorization")}`);
    return api(request);
  }) as typeof fetch;
  const bucket = {
    async head() { return { size: bytes.length }; },
    async get(_key: string, options?: { range?: { offset?: number; length?: number } }) {
      gets++;
      const offset = options?.range?.offset ?? 0;
      const length = options?.range?.length ?? bytes.length;
      return { size: bytes.length, body: new Blob([bytes.slice(offset, offset + length)]).stream() };
    },
  };
  const env: Env = { ASSETS: bucket, MEDIA_API_ORIGIN: "https://api.example", MEDIA_WORKER_SECRET: "secret" };
  return { env, calls, get gets() { return gets; }, restore: () => { globalThis.fetch = originalFetch; } };
}

test("authorizes every viewer and denies before R2", async () => {
  const fixture = setup((request) => request.headers.get("cookie") === "session=allowed"
    ? Response.json({ key: `assets/${id}`, name: "shot.png", contentType: "image/png", byteSize: 10 })
    : new Response(null, { status: 403 }));
  try {
    assert.equal((await handleRequest(new Request(`https://media.test/media/assets/${id}`, { headers: { cookie: "session=allowed" } }), fixture.env)).status, 200);
    assert.equal((await handleRequest(new Request(`https://media.test/media/assets/${id}`, { headers: { cookie: "session=other" } }), fixture.env)).status, 403);
    assert.equal(fixture.gets, 1);
    assert.deepEqual(fixture.calls, ["session=allowed|null", "session=other|null"]);
  } finally { fixture.restore(); }
});

test("revocation is checked on the next request", async () => {
  let allowed = true;
  const fixture = setup(() => allowed ? Response.json({ key: `assets/${id}`, name: "x", contentType: "image/png", byteSize: 10 }) : new Response(null, { status: 404 }));
  try {
    assert.equal((await handleRequest(new Request(`https://x/media/assets/${id}`), fixture.env)).status, 200);
    allowed = false;
    assert.equal((await handleRequest(new Request(`https://x/media/assets/${id}`), fixture.env)).status, 404);
    assert.equal(fixture.gets, 1);
  } finally { fixture.restore(); }
});

test("supports bounded, open, suffix, and unsatisfiable ranges", async () => {
  const fixture = setup();
  try {
    for (const [value, expected, content] of [["bytes=2-4", "bytes 2-4/10", "234"], ["bytes=7-", "bytes 7-9/10", "789"], ["bytes=-3", "bytes 7-9/10", "789"]]) {
      const response = await handleRequest(new Request(`https://x/media/assets/${id}`, { headers: { range: value } }), fixture.env);
      assert.equal(response.status, 206); assert.equal(response.headers.get("content-range"), expected); assert.equal(await response.text(), content);
    }
    const invalid = await handleRequest(new Request(`https://x/media/assets/${id}`, { headers: { range: "bytes=20-30" } }), fixture.env);
    assert.equal(invalid.status, 416); assert.equal(invalid.headers.get("content-range"), "bytes */10");
  } finally { fixture.restore(); }
});

test("HEAD authorizes and does not get object bytes", async () => {
  const fixture = setup();
  try {
    const response = await handleRequest(new Request(`https://x/media/assets/${id}`, { method: "HEAD", headers: { authorization: "Bearer viewer" } }), fixture.env);
    assert.equal(response.status, 200); assert.equal(fixture.gets, 0); assert.equal(fixture.calls[0], "null|Bearer viewer");
  } finally { fixture.restore(); }
});

test("fails closed for redirect, timeout, malformed authorization, and storage mismatch", async () => {
  for (const api of [() => new Response(null, { status: 302, headers: { location: "https://untrusted.example" } }), () => { throw new DOMException("timed out", "TimeoutError"); }, () => Response.json({ key: "wrong", name: "x", contentType: "image/png", byteSize: 10 })]) {
    const fixture = setup(api);
    try { assert.ok((await handleRequest(new Request(`https://x/media/assets/${id}`), fixture.env)).status >= 500); } finally { fixture.restore(); }
  }
  const fixture = setup();
  fixture.env.ASSETS.get = async () => ({ size: 9, body: new Blob([bytes]).stream() });
  try { assert.equal((await handleRequest(new Request(`https://x/media/assets/${id}`), fixture.env)).status, 502); } finally { fixture.restore(); }
});

test("unsafe HTML is an octet-stream attachment with security headers", async () => {
  const fixture = setup(() => Response.json({ key: `assets/${id}`, name: "résumé.html", contentType: "text/html", byteSize: 10 }));
  try {
    const response = await handleRequest(new Request(`https://x/media/assets/${id}`), fixture.env);
    assert.equal(response.headers.get("content-type"), "application/octet-stream");
    assert.match(response.headers.get("content-disposition")!, /^attachment;.*filename\*=UTF-8''r%C3%A9sum%C3%A9\.html$/);
    assert.equal(response.headers.get("cache-control"), "private, no-store");
    assert.equal(response.headers.get("content-security-policy"), "default-src 'none'; sandbox");
  } finally { fixture.restore(); }
});

test("share ID resolves to a distinct asset key; credentials cannot override the Worker secret", async () => {
  const share = "Different123";
  const fixture = setup((request) => {
    assert.equal(new URL(request.url).pathname, `/api/media/shares/${share}`);
    assert.equal(request.headers.get("x-captures-media-key"), fixture.env.MEDIA_WORKER_SECRET);
    assert.equal(request.headers.get("if-none-match"), null);
    assert.equal(request.headers.get("cf-connecting-ip"), null);
    return Response.json({ key: `assets/${id}`, name: "movie.webm", contentType: "video/webm", byteSize: 10 });
  });
  fixture.env.ASSETS.get = async (key) => {
    assert.equal(key, `assets/${id}`);
    return { size: 10, body: new Blob([bytes]).stream() };
  };
  try {
    const response = await handleRequest(new Request(`https://x/media/shares/${share}`, { headers: {
      "x-captures-media-key": "attacker", "if-none-match": "old-etag", "cf-connecting-ip": "127.0.0.1",
    } }), fixture.env);
    assert.equal(response.status, 200);
    assert.equal(await response.text(), "0123456789");
  } finally { fixture.restore(); }
});

test("authorization timeout aborts the fetch and never reaches R2", async () => {
  const fixture = setup((request) => new Promise((_resolve, reject) => {
    request.signal.addEventListener("abort", () => reject(request.signal.reason), { once: true });
  }));
  fixture.env.MEDIA_AUTH_TIMEOUT_MS = "10";
  // AbortSignal.timeout uses an unref timer in Node.
  const keepAlive = setTimeout(() => {}, 1000);
  try {
    const response = await handleRequest(new Request(`https://x/media/assets/${id}`), fixture.env);
    assert.equal(response.status, 504);
    assert.equal(fixture.gets, 0);
  } finally { clearTimeout(keepAlive); fixture.restore(); }
});

test("malformed and multiple ranges fail; HEAD ignores Range; storage failure denies", async () => {
  const fixture = setup();
  try {
    for (const range of ["bytes=-0", "bytes=7-2", "bytes=1-2,4-5", "bytes=+2-4", "items=0-2", "bytes=9007199254740992-"]) {
      assert.equal((await handleRequest(new Request(`https://x/media/assets/${id}`, { headers: { range } }), fixture.env)).status, 416);
    }
    const head = await handleRequest(new Request(`https://x/media/assets/${id}`, { method: "HEAD", headers: { range: "bytes=2-4" } }), fixture.env);
    assert.equal(head.status, 200);
    assert.equal(head.headers.get("content-length"), "10");
    assert.equal(await head.text(), "");
    assert.equal(fixture.gets, 0);
    fixture.env.ASSETS.get = async () => { throw new Error("private storage failure"); };
    const error = await handleRequest(new Request(`https://x/media/assets/${id}`), fixture.env);
    assert.equal(error.status, 502);
    assert.equal(await error.text(), "");
    assert.equal(error.headers.get("cloudflare-cdn-cache-control"), "no-store");
  } finally { fixture.restore(); }
});
