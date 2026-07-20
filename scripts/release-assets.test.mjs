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
  ]) {
    writeFileSync(join(directory, name), name);
  }
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
  assert.equal(result.checksums.length, 9);
  assert.match(readFileSync(join(directory, "SHA256SUMS"), "utf8"), /Captures\.dmg/u);
});

test("rejects an incomplete platform manifest", () => {
  const directory = fixture(["darwin-aarch64", "windows-x86_64"]);
  assert.throws(
    () => validateAndWriteChecksums(directory, "2026.7.1901"),
    /linux-x86_64/u,
  );
});
