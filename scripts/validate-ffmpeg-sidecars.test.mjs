import assert from "node:assert/strict";
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
