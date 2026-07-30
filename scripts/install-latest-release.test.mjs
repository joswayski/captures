import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  expectedChecksum,
  latestPublishedRelease,
  parseOptions,
  platformSpec,
  releaseReadiness,
  verifyChecksum,
} from "./install-latest-release.mjs";

test("selects the newest stable published release", () => {
  const release = latestPublishedRelease([
    {
      id: 3,
      draft: false,
      prerelease: false,
      created_at: "2026-07-29T16:00:00Z",
      published_at: "2026-07-29T17:00:00Z",
    },
    { id: 1, draft: true, prerelease: false, created_at: "2026-07-30T16:00:00Z" },
    {
      id: 2,
      draft: false,
      prerelease: false,
      created_at: "2026-07-28T16:00:00Z",
      published_at: "2026-07-28T17:00:00Z",
    },
    { id: 4, draft: false, prerelease: true, created_at: "2026-07-31T16:00:00Z" },
  ]);
  assert.equal(release.id, 3);
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
