import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { validateBuildConfigurationText } from "./validate-ffmpeg-sidecars.mjs";

const validConfiguration = `
--disable-autodetect
--disable-gpl
--disable-network
--disable-nonfree
--disable-version3
--enable-audiotoolbox
--enable-videotoolbox
--enable-pthreads
--enable-w32threads
--extra-ldflags=-static
--pkg-config-flags=--static
--enable-zlib
`;

test("accepts the pinned LGPL-only FFmpeg configuration", () => {
  for (const platform of ["darwin", "linux", "win32"]) {
    assert.doesNotThrow(() => validateBuildConfigurationText(validConfiguration, platform));
  }
});

test("rejects GPL or nonfree FFmpeg configurations", () => {
  assert.throws(
    () => validateBuildConfigurationText(`${validConfiguration}\n--enable-gpl\n--enable-libx264`),
    /forbidden flag --enable-gpl/u,
  );
});

test("rejects FFmpeg configurations missing a required safety flag", () => {
  assert.throws(
    () => validateBuildConfigurationText(validConfiguration.replace("--disable-network", "")),
    /missing --disable-network/u,
  );
});

test("media cache is exact-key, always validated, and saved before signing", () => {
  const action = readFileSync(new URL("../.github/actions/prepare-media/action.yml", import.meta.url), "utf8");
  const restore = action.indexOf("uses: actions/cache/restore@v4");
  const validate = action.indexOf("run: bash scripts/build-ffmpeg-sidecars.sh");
  const save = action.indexOf("uses: actions/cache/save@v4");
  assert.ok(restore > 0 && restore < validate && validate < save);
  assert.doesNotMatch(action, /restore-keys:|uses: actions\/cache@/u);
  assert.doesNotMatch(action.slice(restore, validate), /\n\s+if:/u);
  for (const input of ["runner.os", "runner.arch", "ImageVersion", "compiler", "rustc", "zlib", "xcrun", "scripts/build-ffmpeg-sidecars.sh", "scripts/validate-ffmpeg-sidecars.mjs", "scripts/download-ffmpeg-source.mjs", "BUILD_CONFIG.txt", "GITHUB_TOKEN"]) {
    assert.ok(action.includes(input), input);
  }
  const paths = [...action.matchAll(/        path: \|\n((?:          .+\n?)+)/gu)].map((match) => match[1].trim());
  assert.equal(paths.length, 2);
  assert.equal(paths[0], paths[1], "restore/save use identical unsigned artifacts");
  const workflow = readFileSync(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");
  assert.ok(workflow.indexOf("uses: ./release-tools/.github/actions/prepare-media") < workflow.indexOf("uses: tauri-apps/tauri-action@v1"));
  assert.match(workflow, /sparse-checkout:[\s\S]*?\.github\/actions\/prepare-media\/action.yml/u);
});
