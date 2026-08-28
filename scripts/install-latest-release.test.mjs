import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  expectedChecksum,
  parseGithubNdjson,
  parseOptions,
  platformSpec,
  releaseReadiness,
  verifyChecksum,
} from "./install-latest-release.mjs";
import { latestPreviewRelease, previewVersion } from "./preview-release.mjs";

test("selects the greatest published Preview version instead of the last one published", () => {
  const release = latestPreviewRelease([
    {
      id: 3,
      draft: false,
      prerelease: true,
      tag_name: "v2026.07.31.1",
      published_at: "2026-07-31T17:00:00Z",
    },
    {
      id: 4,
      draft: false,
      prerelease: true,
      tag_name: "v2026.07.29.17",
      published_at: "2026-08-01T17:00:00Z",
    },
    {
      id: 5,
      draft: false,
      prerelease: false,
      tag_name: "v2026.08.01.1",
      published_at: "2026-08-01T18:00:00Z",
    },
    {
      id: 6,
      draft: false,
      prerelease: true,
      tag_name: "preview",
      published_at: "2026-08-01T19:00:00Z",
    },
  ]);
  assert.equal(release.id, 3);
  assert.deepEqual(previewVersion(release.tag_name), [2026, 7, 31, 1]);
});

test("requires both the system installer and completed-release marker", () => {
  const spec = platformSpec("darwin", "arm64");
  const building = releaseReadiness(
    {
      assets: [{ name: "Captures_2026.7.2901_aarch64.dmg", state: "uploaded" }],
    },
    spec,
  );
  assert.equal(building.ready, false);
  assert.deepEqual(building.missing, ["complete-release validation"]);

  const complete = releaseReadiness(
    {
      assets: [
        { name: "Captures_2026.7.2901_aarch64.dmg", state: "uploaded" },
        { name: "SHA256SUMS", state: "uploaded" },
      ],
    },
    spec,
  );
  assert.equal(complete.ready, true);
  assert.equal(complete.installAsset.name, "Captures_2026.7.2901_aarch64.dmg");
});

test("maps supported systems to their native release package", () => {
  const mac = platformSpec("darwin", "arm64");
  const windows = platformSpec("win32", "x64");
  const debian = platformSpec("linux", "x64", true);
  const appImage = platformSpec("linux", "x64");

  assert.equal(mac.packageType, "dmg");
  assert.equal(mac.matches("Captures_2026.7.2901_aarch64.dmg"), true);
  assert.equal(windows.packageType, "nsis");
  assert.equal(windows.matches("Captures_2026.7.2901_x64-setup.exe"), true);
  assert.equal(windows.matches("Captures_2026.7.2901_x64-setup.exe.sig"), false);
  assert.equal(debian.packageType, "deb");
  assert.equal(debian.matches("Captures_2026.7.2901_amd64.deb"), true);
  assert.equal(appImage.packageType, "appimage");
  assert.equal(appImage.matches("Captures_2026.7.2901_amd64.AppImage"), true);
  assert.throws(() => platformSpec("darwin", "x64"), /no official Captures installer/u);
});

test("parses command options without allowing unknown behavior", () => {
  assert.deepEqual(parseOptions(["--no-launch", "--no-wait"]), {
    dryRun: false,
    launch: false,
    waitMs: 0,
  });
  assert.deepEqual(parseOptions(["--dry-run"]), {
    dryRun: true,
    launch: true,
    waitMs: 0,
  });
  assert.throws(() => parseOptions(["--latest-complete"]), /unknown option/u);
});

test("verifies the downloaded installer against SHA256SUMS", () => {
  const directory = mkdtempSync(join(tmpdir(), "captures-install-test-"));
  try {
    const assetName = "Captures.dmg";
    const assetPath = join(directory, assetName);
    const manifestPath = join(directory, "SHA256SUMS");
    writeFileSync(assetPath, "signed release");
    const checksum = createHash("sha256").update(readFileSync(assetPath)).digest("hex");
    writeFileSync(manifestPath, `${checksum}  ${assetName}\n`);

    assert.equal(expectedChecksum(readFileSync(manifestPath, "utf8"), assetName), checksum);
    assert.equal(verifyChecksum(assetPath, manifestPath), checksum);

    writeFileSync(assetPath, "tampered release");
    assert.throws(() => verifyChecksum(assetPath, manifestPath), /checksum mismatch/u);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("parses GitHub paginated NDJSON without loading full release bodies", () => {
  assert.deepEqual(parseGithubNdjson(""), []);
  assert.deepEqual(parseGithubNdjson("\n"), []);
  assert.deepEqual(
    parseGithubNdjson([
      '{"id":3,"tag_name":"v2026.07.31.1","draft":false,"prerelease":true,"published_at":"2026-07-31T17:00:00Z","assets":[{"name":"Captures.dmg","state":"uploaded"}]}',
      '{"id":6,"tag_name":"preview","draft":false,"prerelease":true,"published_at":"2026-08-01T19:00:00Z","assets":[]}',
    ].join("\n")),
    [
      {
        id: 3,
        tag_name: "v2026.07.31.1",
        draft: false,
        prerelease: true,
        published_at: "2026-07-31T17:00:00Z",
        assets: [{ name: "Captures.dmg", state: "uploaded" }],
      },
      {
        id: 6,
        tag_name: "preview",
        draft: false,
        prerelease: true,
        published_at: "2026-08-01T19:00:00Z",
        assets: [],
      },
    ],
  );
  assert.throws(() => parseGithubNdjson("{not json}"), /invalid JSON on line 1/u);

  const slim = parseGithubNdjson([
    '{"id":3,"name":"Captures Preview 2026.07.31.1","tag_name":"v2026.07.31.1","draft":false,"prerelease":true,"published_at":"2026-07-31T17:00:00Z","created_at":"2026-07-31T16:00:00Z","assets":[{"name":"Captures_2026.7.3101_aarch64.dmg","state":"uploaded"},{"name":"SHA256SUMS","state":"uploaded"}]}',
    '{"id":6,"name":"Captures Preview — Latest","tag_name":"preview","draft":false,"prerelease":true,"published_at":"2026-08-01T19:00:00Z","created_at":"2026-08-01T19:00:00Z","assets":[]}',
  ].join("\n"));
  const release = latestPreviewRelease(slim);
  assert.equal(release.id, 3);
  const readiness = releaseReadiness(release, platformSpec("darwin", "arm64"));
  assert.equal(readiness.ready, true);
  assert.equal(readiness.installAsset.name, "Captures_2026.7.3101_aarch64.dmg");
});
