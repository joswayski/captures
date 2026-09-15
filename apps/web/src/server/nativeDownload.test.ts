import assert from "node:assert/strict";
import test from "node:test";
import { nativeDownload } from "./nativeDownload.ts";

test("downloads follow one published immutable set, retaining it across staging and failures", async (t) => {
  let now = Date.now();
  let calls = 0;
  let status = 404;
  let channel = { draft: false, prerelease: true, body: "" };
  t.mock.method(Date, "now", () => now);
  t.mock.method(globalThis, "fetch", async (url: string) => {
    assert.equal(url, "https://api.github.com/repos/joswayski/captures/releases/tags/native-preview");
    calls++;
    return new Response(JSON.stringify(channel), { status });
  });
  const filename = "Captures-macOS-Apple-Silicon.dmg";
  const base = "https://github.com/joswayski/captures/releases/download/";
  const response = await nativeDownload(filename);
  assert.equal(response.status, 503);
  assert.equal(response.headers.get("Cache-Control"), "no-store");

  status = 200;
  now += 60_001;
  channel.body = "<!-- native-preview-tag: native-v2026.09.13.1 -->";
  const before = calls;
  const assets = [filename, "Captures-Windows-x64-setup.exe", "Captures-Linux-x64.deb", "Captures-Linux-x64.tar.gz"];
  const downloads = await Promise.all(assets.map(nativeDownload));
  assert.equal(calls, before + 1, "parallel downloads share one channel lookup");
  for (const [index, download] of downloads.entries()) {
    assert.equal(download.status, 302);
    assert.equal(download.headers.get("Location"), `${base}native-v2026.09.13.1/${assets[index]}`);
  }

  for (const invalid of [
    { draft: true, prerelease: true, body: "<!-- native-preview-tag: native-v2026.09.14.1 -->" },
    { draft: false, prerelease: true, body: "<!-- native-preview-tag: https://evil.example -->" },
    { draft: false, prerelease: true, body: "<!-- native-preview-tag: v2026.09.14.1 -->" },
  ]) {
    now += 300_001;
    channel = invalid;
    assert.equal((await nativeDownload(filename)).headers.get("Location"), `${base}native-v2026.09.13.1/${filename}`);
  }
  now += 300_001;
  status = 502;
  assert.equal((await nativeDownload(filename)).headers.get("Location"), `${base}native-v2026.09.13.1/${filename}`);

  now += 60_001;
  status = 200;
  channel = { draft: false, prerelease: true, body: "<!-- native-preview-tag: native-v2026.09.14.1 -->" };
  for (const asset of assets) {
    assert.equal((await nativeDownload(asset)).headers.get("Location"), `${base}native-v2026.09.14.1/${asset}`);
  }
  const lookups = calls;
  for (const asset of ["latest.json", "../other", "Captures-Linux-x64.AppImage"]) {
    assert.equal((await nativeDownload(asset)).status, 404);
  }
  assert.equal(calls, lookups);
});
