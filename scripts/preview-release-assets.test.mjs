import assert from "node:assert/strict";
import test from "node:test";

import { previewReleaseAssets } from "./preview-release-assets.mjs";

const completeAssets = [
  "Captures_2026.7.3103_aarch64.dmg",
  "Captures_2026.7.3103_x64-setup.exe",
  "Captures_2026.7.3103_amd64.deb",
  "Captures_2026.7.3103_amd64.AppImage",
  "latest.json",
];

test("selects one human installer for each supported Preview platform", () => {
  assert.deepEqual(previewReleaseAssets(completeAssets), {
    macos: "Captures_2026.7.3103_aarch64.dmg",
    windows: "Captures_2026.7.3103_x64-setup.exe",
    linuxDeb: "Captures_2026.7.3103_amd64.deb",
    linuxAppImage: "Captures_2026.7.3103_amd64.AppImage",
    updater: "latest.json",
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
