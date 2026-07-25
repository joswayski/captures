import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { validateAndWriteChecksums } from "./release-assets.mjs";

function fixture(platforms = ["darwin-aarch64", "windows-x86_64", "linux-x86_64"]) {
  const directory = mkdtempSync(join(tmpdir(), "captures-release-assets-"));
  for (const name of [
    "Captures.dmg",
    "Captures.app.tar.gz",
    "Captures.app.tar.gz.sig",
    "Captures-setup.exe",
    "Captures-setup.exe.sig",
    "Captures.AppImage",
    "Captures.AppImage.sig",
    "Captures.deb",
    "ffmpeg-8.1.2.tar.xz",
    "ffmpeg-8.1.2.tar.xz.asc",
    "ffmpeg-8.1.2-COPYING.LGPLv2.1",
    "ffmpeg-8.1.2-NOTICE.md",
  ]) {
    writeFileSync(join(directory, name), name);
  }
  writeFileSync(
    join(directory, "ffmpeg-8.1.2-BUILD_CONFIG.txt"),
    "--disable-gpl\n--disable-nonfree\n--disable-version3\n",
  );
  writeFileSync(
    join(directory, "latest.json"),
    JSON.stringify({
      version: "2026.7.1901",
      notes: "Adds automated releases.",
      platforms: Object.fromEntries(
        platforms.map((platform) => [platform, { url: `https://example.com/${platform}`, signature: "signed" }]),
      ),
    }),
  );
  return directory;
}

test("validates complete updater metadata and writes deterministic checksums", () => {
  const directory = fixture();
  const result = validateAndWriteChecksums(directory, "2026.7.1901");
  assert.equal(result.checksums.length, 14);
  assert.match(readFileSync(join(directory, "SHA256SUMS"), "utf8"), /Captures\.dmg/u);
});

test("rejects a release without matching FFmpeg compliance assets", () => {
  const directory = fixture();
  writeFileSync(join(directory, "ffmpeg-8.1.2-BUILD_CONFIG.txt"), "--enable-gpl\n--enable-libx264\n");
  assert.throws(
    () => validateAndWriteChecksums(directory, "2026.7.1901"),
    /missing --disable-gpl/u,
  );
});

test("rejects an incomplete platform manifest", () => {
  const directory = fixture(["darwin-aarch64", "windows-x86_64"]);
  assert.throws(
    () => validateAndWriteChecksums(directory, "2026.7.1901"),
    /linux-x86_64/u,
  );
});
