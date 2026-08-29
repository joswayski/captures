import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { validateAndWriteChecksums, rewriteGithubApiAssetUrls } from "./release-assets.mjs";

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

test("rewrites GitHub API updater URLs to public Releases downloads", () => {
  const latest = {
    platforms: {
      "darwin-aarch64": {
        url: "https://api.github.com/repos/joswayski/captures/releases/assets/11",
        signature: "mac",
      },
      "windows-x86_64": {
        url: "https://api.github.com/repos/joswayski/captures/releases/assets/22",
        signature: "win",
      },
      "linux-x86_64": {
        url: "https://github.com/joswayski/captures/releases/download/v2026.08.29.4/Captures.AppImage",
        signature: "linux",
      },
    },
  };
  const rewritten = rewriteGithubApiAssetUrls(latest, [
    {
      id: 11,
      browser_download_url:
        "https://github.com/joswayski/captures/releases/download/v2026.08.29.4/Captures.app.tar.gz",
    },
    {
      id: 22,
      browser_download_url:
        "https://github.com/joswayski/captures/releases/download/v2026.08.29.4/Captures-setup.exe",
    },
  ]);
  assert.equal(rewritten, 2);
  assert.equal(
    latest.platforms["darwin-aarch64"].url,
    "https://github.com/joswayski/captures/releases/download/v2026.08.29.4/Captures.app.tar.gz",
  );
  assert.equal(
    latest.platforms["windows-x86_64"].url,
    "https://github.com/joswayski/captures/releases/download/v2026.08.29.4/Captures-setup.exe",
  );
  assert.equal(
    latest.platforms["linux-x86_64"].url,
    "https://github.com/joswayski/captures/releases/download/v2026.08.29.4/Captures.AppImage",
  );
});

test("rejects updater manifests that still use GitHub API asset URLs", () => {
  const directory = fixture();
  writeFileSync(
    join(directory, "latest.json"),
    JSON.stringify({
      version: "2026.7.1901",
      notes: "Adds automated releases.",
      platforms: {
        "darwin-aarch64": {
          url: "https://api.github.com/repos/joswayski/captures/releases/assets/11",
          signature: "signed",
        },
        "windows-x86_64": { url: "https://example.com/windows", signature: "signed" },
        "linux-x86_64": { url: "https://example.com/linux", signature: "signed" },
      },
    }),
  );
  assert.throws(
    () => validateAndWriteChecksums(directory, "2026.7.1901"),
    /GitHub API/u,
  );
});
