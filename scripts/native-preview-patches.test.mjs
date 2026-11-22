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
  // The newest builds, then the last build of each earlier day.
  const published = (builds) =>
    builds.map((build) =>
      release(`v2026.11.${Math.floor(build / 100)}.${build % 100}`, [
        mac(`2026.11.${build}`),
        "captures-native.json",
      ]),
    );
  const range = (first, count, step = 1) =>
    Array.from({ length: count }, (_, index) => first + index * step);
  const many = published([...range(2101, 10), ...range(2001, 12), ...range(1001, 10, 100)]);
  const versions = (releases) =>
    baseReleases(releases, "aarch64-apple-darwin", "2026.11.2199", false).map((base) =>
      Number(base.version.split(".")[2]),
    );
  assert.deepEqual(versions(many), [
    ...range(2110, 8, -1),
    ...[2012, 1901, 1801, 1701, 1601, 1501, 1401],
  ]);
  // Without enough earlier days, the next newest builds fill the rest.
  assert.deepEqual(versions(published([...range(2101, 20), 2001])), [...range(2120, 14, -1), 2001]);
  // Other targets' artifacts in the same releases are not bases.
  assert.deepEqual(baseReleases(many, "x86_64-pc-windows-msvc", "2026.11.2199", false), []);
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
