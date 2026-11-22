import assert from "node:assert/strict";
import { test } from "vitest";

import { BASES, baseReleases, installerOnly } from "./native-preview-patches.mjs";

const release = (tag, names, extra = {}) => ({
  tag_name: tag,
  draft: false,
  assets: names.map((name) => ({ name, state: "uploaded" })),
  ...extra,
});
const mac = (version) => `Captures-${version}-aarch64-apple-darwin.app.tar.gz`;

test("patches start from the newest earlier native builds of the same feed", () => {
  const releases = [
    release("v2026.11.21.10", [mac("2026.11.2110"), "captures-native.json"]),
    release("v2026.11.21.9", [mac("2026.11.2109"), "captures-native.json"]),
    release("v2026.11.09.1", [mac("2026.11.901"), "captures-native.json"]),
    // Tauri and unmarked October native releases cannot apply patches.
    release("v2026.11.21.8", [mac("2026.11.2108"), "captures-tauri.json"]),
    release("v2026.10.18.1", [mac("2026.10.1801")]),
    release("v2026.11.21.7", [mac("2026.11.2107"), "captures-native.json"], { draft: true }),
    release("native-staging-2026.11.2105", [mac("2026.11.2105")]),
    release("v2026.11.21.11", [mac("2026.11.2111"), "captures-native.json"]),
  ];
  assert.deepEqual(
    baseReleases(releases, "aarch64-apple-darwin", "2026.11.2111", false).map(
      (base) => base.version,
    ),
    ["2026.11.2110", "2026.11.2109", "2026.11.901"],
  );
  assert.deepEqual(baseReleases(releases, "aarch64-apple-darwin", "2026.11.2111", true), [
    {
      tag: "native-staging-2026.11.2105",
      name: mac("2026.11.2105"),
      version: "2026.11.2105",
    },
  ]);
  // Clients consider at most 16 offers.
  assert.equal(BASES, 15);
  const many = Array.from({ length: 20 }, (_, index) =>
    release(`v2026.11.10.${index + 1}`, [mac(`2026.11.${1001 + index}`), "captures-native.json"]),
  );
  const chosen = baseReleases(many, "aarch64-apple-darwin", "2026.11.1100", false);
  assert.equal(chosen.length, 15);
  assert.deepEqual([chosen[0].version, chosen.at(-1).version], ["2026.11.1020", "2026.11.1006"]);
  // Other targets' artifacts in the same releases are not bases.
  assert.deepEqual(baseReleases(many, "x86_64-pc-windows-msvc", "2026.11.1100", false), []);
});

test("Windows bases keep only what the installer writes to the install folder", () => {
  assert.deepEqual(
    [
      "captures.exe",
      "ffmpeg.exe",
      "notices",
      "$PLUGINSDIR",
      "[NSIS].nsi",
      "Uninstall.exe",
      "native_preview_update_helper.exe",
    ].filter((name) => !installerOnly(name)),
    ["captures.exe", "ffmpeg.exe", "notices", "native_preview_update_helper.exe"],
  );
});
