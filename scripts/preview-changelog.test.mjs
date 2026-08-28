import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { applyPreviewChangelog, previewChangelog } from "./preview-changelog.mjs";
import { encodedPreviewVersion } from "./preview-release.mjs";

function release({ tag, body, draft = false, prerelease = true, id = 1 }) {
  return { tag_name: tag, body, draft, prerelease, id };
}

test("encodes dated Preview tags as Tauri SemVer and display CalVer", () => {
  assert.deepEqual(encodedPreviewVersion("v2026.08.27.5"), {
    version: "2026.8.2705",
    displayVersion: "2026.08.27.5",
    order: [2026, 8, 27, 5],
  });
  assert.equal(encodedPreviewVersion("preview"), null);
});

test("stacks dated Preview notes newest first and ignores other releases", () => {
  const changelog = previewChangelog([
    release({ tag: "preview", body: "Channel page", id: 99 }),
    release({ tag: "v2026.08.27.4", body: "Four", id: 4 }),
    release({ tag: "v2026.08.27.5", body: "Five", id: 5 }),
    release({ tag: "v2026.08.27.3", body: "Draft three", draft: true, id: 3 }),
    release({ tag: "v1.2.3", body: "Stable", prerelease: false, id: 2 }),
    release({ tag: "v2026.08.27.2", body: "Two", id: 2 }),
  ]);

  assert.deepEqual(changelog, [
    { version: "2026.8.2705", display_version: "2026.08.27.5", notes: "Five" },
    { version: "2026.8.2704", display_version: "2026.08.27.4", notes: "Four" },
    { version: "2026.8.2702", display_version: "2026.08.27.2", notes: "Two" },
  ]);
});

test("rejects a manifest that is not the newest changelog version", () => {
  assert.throws(
    () => applyPreviewChangelog(
      { version: "2026.8.2704", notes: "Four", platforms: {} },
      [release({ tag: "v2026.08.27.5", body: "Five" })],
    ),
    /does not match newest changelog 2026\.8\.2705/u,
  );
});

test("writes changelog onto latest.json for the Preview channel", () => {
  const directory = mkdtempSync(join(tmpdir(), "captures-preview-changelog-"));
  const latestPath = join(directory, "latest.json");
  const releasesPath = join(directory, "releases.json");
  writeFileSync(latestPath, JSON.stringify({
    version: "2026.8.2705",
    notes: "Five",
    platforms: { "darwin-aarch64": { url: "https://example.com", signature: "sig" } },
  }));
  writeFileSync(releasesPath, JSON.stringify([
    release({ tag: "v2026.08.27.4", body: "Four" }),
    release({ tag: "v2026.08.27.5", body: "Five" }),
  ]));

  const result = spawnSync(
    process.execPath,
    [fileURLToPath(new URL("./preview-changelog.mjs", import.meta.url)), "apply", latestPath, releasesPath],
    { encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);

  const updated = JSON.parse(readFileSync(latestPath, "utf8"));
  assert.equal(updated.version, "2026.8.2705");
  assert.equal(updated.notes, "Five");
  assert.deepEqual(updated.changelog, [
    { version: "2026.8.2705", display_version: "2026.08.27.5", notes: "Five" },
    { version: "2026.8.2704", display_version: "2026.08.27.4", notes: "Four" },
  ]);
});
