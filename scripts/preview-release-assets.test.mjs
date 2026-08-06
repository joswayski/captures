import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  PREVIEW_CHANNEL_ASSET_NAMES,
  preparePreviewChannelAssets,
  previewReleaseAssets,
} from "./preview-release-assets.mjs";

const completeAssets = [
  "Captures_2026.7.3103_aarch64.dmg",
  "Captures_2026.7.3103_x64-setup.exe",
  "Captures_2026.7.3103_amd64.deb",
  "Captures_2026.7.3103_amd64.AppImage",
  "latest.json",
];

const stableAssets = Object.values(PREVIEW_CHANNEL_ASSET_NAMES);

function writeAssetDir(names) {
  const directory = mkdtempSync(join(tmpdir(), "captures-preview-assets-"));
  for (const name of names) {
    writeFileSync(join(directory, name), name);
  }
  return directory;
}

test("selects one human installer for each supported Preview platform", () => {
  assert.deepEqual(previewReleaseAssets(completeAssets), {
    macos: "Captures_2026.7.3103_aarch64.dmg",
    windows: "Captures_2026.7.3103_x64-setup.exe",
    linuxDeb: "Captures_2026.7.3103_amd64.deb",
    linuxAppImage: "Captures_2026.7.3103_amd64.AppImage",
    updater: "latest.json",
  });
});

test("selects stable Preview channel names when already renamed", () => {
  assert.deepEqual(previewReleaseAssets(stableAssets), {
    ...PREVIEW_CHANNEL_ASSET_NAMES,
  });
});

test("rejects incomplete, ambiguous, or technical-only Preview assets", () => {
  assert.throws(
    () => previewReleaseAssets(completeAssets.filter((name) => !name.endsWith(".dmg"))),
    /exactly one macos asset; found 0/u,
  );
  assert.throws(
    () => previewReleaseAssets([...completeAssets, "Captures_other_aarch64.dmg"]),
    /exactly one macos asset; found 2/u,
  );
  assert.throws(
    () => previewReleaseAssets([...completeAssets, "SHA256SUMS"]),
    /unexpected assets: SHA256SUMS/u,
  );
});

test("prepares a Preview channel directory with stable asset names", () => {
  const directory = writeAssetDir(completeAssets);
  const prepared = preparePreviewChannelAssets(directory);

  assert.deepEqual(prepared, { ...PREVIEW_CHANNEL_ASSET_NAMES });
  assert.deepEqual(readdirSync(directory).sort(), stableAssets.slice().sort());
});

test("prepare is a no-op when assets already use stable names", () => {
  const directory = writeAssetDir(stableAssets);
  const prepared = preparePreviewChannelAssets(directory);

  assert.deepEqual(prepared, { ...PREVIEW_CHANNEL_ASSET_NAMES });
  assert.deepEqual(readdirSync(directory).sort(), stableAssets.slice().sort());
});

test("prepare rejects non-file entries", () => {
  const directory = writeAssetDir(completeAssets.slice(0, 4));
  mkdirSync(join(directory, "nested"));
  assert.throws(() => preparePreviewChannelAssets(directory), /not a file/u);
});
