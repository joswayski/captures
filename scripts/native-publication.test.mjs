import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { NATIVE_ASSETS, validateNativeAssets } from "./native-release-assets.mjs";

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "native-publish-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const directory of ["bin", "scripts", "native-dist"]) mkdirSync(join(root, directory));
  for (const asset of NATIVE_ASSETS) writeFileSync(join(root, "native-dist", asset), "abc");
  writeFileSync(join(root, "native-dist/SHA256SUMS"), validateNativeAssets(join(root, "native-dist")));
  writeFileSync(join(root, "channel-body.txt"), "<!-- native-preview-tag: native-v2026.09.13.1 -->");
  writeFileSync(join(root, "bin/gh"), `#!/usr/bin/env node
const fs = require('node:fs');
const args = process.argv.slice(2);
fs.appendFileSync('calls.jsonl', JSON.stringify(args) + '\\n');
if (args.includes('--paginate')) {
  if (process.env.FAIL_QUERY) process.exit(1);
  process.stdout.write(JSON.stringify(process.env.NO_CHANNEL ? [] : [{id: 42, tag_name: 'native-preview'}, {id: 7, tag_name: 'preview'}]));
} else if (args.includes('POST')) {
  process.stdout.write(args.includes('tag_name=native-preview') ? '42' : '100');
} else if (args.includes('PATCH')) {
  if (args.includes('repos/fixture/captures/releases/42')) {
    if (process.env.FAIL_CHANNEL_PATCH) process.exit(1);
    fs.writeFileSync('channel-body.txt', args.find(arg => arg.startsWith('body=')).slice(5));
  } else if (process.env.FAIL_PUBLISH) process.exit(1);
}
`, { mode: 0o755 });
  writeFileSync(join(root, "scripts/github-release-assets.mjs"), `
import { appendFileSync, cpSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
appendFileSync('calls.jsonl', JSON.stringify([process.argv[2], process.argv[3]]) + '\\n');
if (process.argv[2] === 'sync' && process.env.FAIL_SYNC) process.exit(1);
if (process.argv[2] === 'download') {
  cpSync('native-dist', process.argv[4], { recursive: true });
  if (process.env.CORRUPT_DOWNLOAD) writeFileSync(join(process.argv[4], 'Captures-Linux-x64.deb'), 'corrupt');
}
`);
  return {
    root,
    run(extra = {}) {
      const result = spawnSync("bash", [fileURLToPath(new URL("./publish-native-release.sh", import.meta.url))], {
        cwd: root,
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${join(root, "bin")}${delimiter}${process.env.PATH}`,
          GITHUB_REPOSITORY: "fixture/captures",
          GITHUB_SHA: "a".repeat(40),
          NATIVE_TAG: "native-v2026.09.14.1",
          ...extra,
        },
      });
      return result;
    },
    calls() { return readFileSync(join(root, "calls.jsonl"), "utf8").trim().split("\n").map(JSON.parse); },
  };
}

test("publishes verified immutable assets before switching only the channel pointer", (t) => {
  const f = fixture(t);
  const result = f.run();
  assert.equal(result.status, 0, result.stderr);
  const calls = f.calls();
  assert.deepEqual(calls.filter((call) => call[0] === "sync"), [["sync", "100"]]);
  const publish = calls.filter((call) => call.includes("PATCH"));
  assert.equal(publish.length, 2);
  assert.ok(publish[0].includes("repos/fixture/captures/releases/100"));
  assert.ok(publish[1].includes("repos/fixture/captures/releases/42"));
  assert.ok(calls.indexOf(publish[0]) > calls.findIndex((call) => call[0] === "download"));
  const body = readFileSync(join(f.root, "channel-body.txt"), "utf8");
  assert.match(body, /<!-- native-preview-tag: native-v2026.09.14.1 -->/);
  for (const asset of NATIVE_ASSETS) {
    assert.ok(body.includes(`/releases/download/native-v2026.09.14.1/${asset}`));
  }
  for (const call of publish) {
    assert.ok(call.includes("prerelease=true"));
    assert.ok(call.includes("make_latest=false"));
  }
  assert.ok(!calls.some((call) => call.includes("repos/fixture/captures/releases/7")));
});

test("failed archive upload never publishes or changes the download channel", (t) => {
  const f = fixture(t);
  assert.notEqual(f.run({ FAIL_SYNC: "1" }).status, 0);
  assert.deepEqual(f.calls().filter((call) => call[0] === "sync"), [["sync", "100"]]);
  assert.ok(!f.calls().some((call) => call.includes("PATCH")));
});

test("failed GitHub listing never creates a replacement release", (t) => {
  const f = fixture(t);
  assert.notEqual(f.run({ FAIL_QUERY: "1" }).status, 0);
  assert.equal(f.calls().length, 1);
});

test("downloaded package bytes must match local checksums before publication", (t) => {
  const f = fixture(t);
  const result = f.run({ CORRUPT_DOWNLOAD: "1" });
  assert.notEqual(result.status, 0);
  assert.match(result.stdout, /Captures-Linux-x64.deb: FAILED/);
  assert.ok(!f.calls().some((call) => call.includes("PATCH")));
});

for (const failure of ["FAIL_SYNC", "CORRUPT_DOWNLOAD", "FAIL_PUBLISH", "FAIL_CHANNEL_PATCH"]) {
  test(`${failure} leaves the previous public download set untouched`, (t) => {
    const f = fixture(t);
    assert.notEqual(f.run({ [failure]: "1" }).status, 0);
    assert.equal(readFileSync(join(f.root, "channel-body.txt"), "utf8"),
      "<!-- native-preview-tag: native-v2026.09.13.1 -->");
    assert.deepEqual(f.calls().filter((call) => call[0] === "sync"), [["sync", "100"]]);
  });
}

test("first publication creates a pointer-only channel after verifying the archive", (t) => {
  const f = fixture(t);
  const result = f.run({ NO_CHANNEL: "1" });
  assert.equal(result.status, 0, result.stderr);
  const calls = f.calls();
  const channelCreation = calls.findIndex((call) => call.includes("tag_name=native-preview"));
  const datedPublish = calls.findIndex((call) => call.includes("PATCH") && call.includes("repos/fixture/captures/releases/100"));
  assert.ok(channelCreation > datedPublish);
  assert.deepEqual(calls.filter((call) => call[0] === "sync"), [["sync", "100"]]);
});
