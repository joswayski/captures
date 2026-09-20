import assert from "node:assert/strict";
import test from "node:test";
import {
  assetMediaKind,
  uploadAsset,
  validateShare,
  validateUpload,
} from "./accountModel.ts";

test("uploads accept arbitrary non-empty files", () => {
  assert.equal(validateUpload({ size: 42 }), null);
  assert.match(validateUpload({ size: 0 })!, /empty/);
  assert.equal(assetMediaKind("image/gif"), "image");
  assert.equal(assetMediaKind("video/mp4"), "video");
  assert.equal(assetMediaKind("image/svg+xml"), "download");
  assert.equal(assetMediaKind("application/pdf"), "download");
});

test("share validation checks replacement password and future expiry", () => {
  assert.match(validateShare("short", "")!, /8/);
  assert.match(validateShare("", "2000-01-01T00:00")!, /future/);
  assert.equal(validateShare("eight-ok", "2999-01-01T00:00"), null);
});

test("multipart upload sends bounded slices directly to R2 and completes with ETags", async () => {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const asset = {
    id: "asset-1",
    name: "clip.bin",
    contentType: "application/octet-stream",
    byteSize: 6,
    createdAt: "2026-01-01T00:00:00Z",
    share: null,
  };
  const fetcher = async (
    input: string | URL | Request,
    init: RequestInit = {},
  ) => {
    const url = String(input);
    calls.push({ url, init });
    if (url === "/api/assets" && init.method === "POST")
      return Response.json(
        { id: "asset-1", partSize: 3, partCount: 2 },
        { status: 201 },
      );
    if (url.endsWith("/parts")) {
      const partNumber = JSON.parse(String(init.body)).partNumber;
      return Response.json({
        url: `https://r2.example/${partNumber}`,
        headers: { "x-upload-token": `p${partNumber}` },
      });
    }
    if (url.startsWith("https://r2.example/"))
      return new Response(null, {
        status: 200,
        headers: { ETag: `\"etag-${url.at(-1)}\"` },
      });
    if (url.endsWith("/complete")) return Response.json(asset);
    throw new Error(`Unexpected ${url}`);
  };
  const progress: number[] = [];
  const result = await uploadAsset(new File(["abcdef"], "clip.bin"), {
    signal: new AbortController().signal,
    fetcher: fetcher as typeof fetch,
    onProgress: (done) => progress.push(done),
  });
  assert.deepEqual(result, asset);
  assert.deepEqual(progress, [1, 2]);
  const puts = calls.filter((call) => call.init.method === "PUT");
  assert.equal(puts.length, 2);
  assert.deepEqual(
    puts.map((call) => (call.init.body as Blob).size),
    [3, 3],
  );
  assert.deepEqual(
    await Promise.all(puts.map((call) => (call.init.body as Blob).text())),
    ["abc", "def"],
  );
  assert.deepEqual(
    puts.map((call) => call.init.credentials),
    ["omit", "omit"],
  );
  assert.deepEqual(puts[0].init.headers, { "x-upload-token": "p1" });
  const completion = calls.find((call) => call.url.endsWith("/complete"))!;
  assert.deepEqual(JSON.parse(String(completion.init.body)), {
    parts: [
      { partNumber: 1, etag: '"etag-1"' },
      { partNumber: 2, etag: '"etag-2"' },
    ],
  });
});

test("multipart upload never completes when R2 does not expose ETag", async () => {
  const urls: string[] = [];
  const fetcher = async (
    input: string | URL | Request,
    init: RequestInit = {},
  ) => {
    const url = String(input);
    urls.push(url);
    if (url === "/api/assets")
      return Response.json(
        { id: "a", partSize: 10, partCount: 1 },
        { status: 201 },
      );
    if (url.endsWith("/parts"))
      return Response.json({ url: "https://r2.example/1", headers: {} });
    if (init.method === "PUT") return new Response(null, { status: 200 });
    throw new Error("complete must not be called");
  };
  await assert.rejects(
    uploadAsset(new File(["x"], "x"), {
      signal: new AbortController().signal,
      fetcher: fetcher as typeof fetch,
    }),
    /ETag/,
  );
  assert.equal(
    urls.some((url) => url.endsWith("/complete")),
    false,
  );
});

test("cancelling a direct upload aborts the pending asset with a fresh signal", async () => {
  const controller = new AbortController();
  let cleaned = false;
  const fetcher = async (
    input: string | URL | Request,
    init: RequestInit = {},
  ) => {
    const url = String(input);
    if (url === "/api/assets")
      return Response.json({ id: "abcdefghijkl", partSize: 10, partCount: 1 });
    if (url.endsWith("/parts"))
      return Response.json({ url: "https://r2.example/1", headers: {} });
    if (init.method === "DELETE") {
      assert.equal(init.signal?.aborted, false);
      assert.equal(init.credentials, "same-origin");
      cleaned = true;
      return new Response(null, { status: 204 });
    }
    if (init.method === "PUT") {
      controller.abort();
      throw new DOMException("Cancelled", "AbortError");
    }
    throw new Error("Completion must not run");
  };
  await assert.rejects(
    uploadAsset(new File(["abc"], "clip.webm"), {
      signal: controller.signal,
      fetcher: fetcher as typeof fetch,
    }),
    /Cancelled/,
  );
  assert.equal(cleaned, true);
});
