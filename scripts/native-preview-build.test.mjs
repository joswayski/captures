import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "vitest";

import {
  bundleConfiguration,
  manifest,
  TARGETS,
  updaterArtifactName,
} from "./native-preview-build.mjs";

test("native packages preserve media and installed identity without a frontend or WebView installer", () => {
  const windows = bundleConfiguration("x86_64-pc-windows-msvc", "2026.10.1101");
  assert.equal(windows.version, "2026.10.1101");
  assert.equal(windows.build.frontendDist, null);
  assert.equal(windows.build.beforeBuildCommand, null);
  assert.deepEqual(windows.bundle.windows.webviewInstallMode, { type: "skip" });
  assert.deepEqual(windows.bundle.externalBin, [
    "binaries/ffmpeg",
    "binaries/ffprobe",
    "binaries/native_preview_update_helper",
  ]);
  assert.equal(windows.bundle.resources.ffmpeg, "notices/ffmpeg");
  const mac = bundleConfiguration("aarch64-apple-darwin", "2026.10.1101");
  assert.ok(Object.values(mac.bundle.resources).includes("CapturesNative_CapturesNative.bundle"));
  assert.match(mac.bundle.macOS.infoPlist, /native-preview\/Info\.plist$/u);
  assert.throws(() => bundleConfiguration("x86_64-pc-windows-msvc", "../other"), /Invalid/u);
});

test("the shared updater manifest requires every version-bound native artifact and uses legacy platform keys", () => {
  const directory = mkdtempSync(join(tmpdir(), "captures-native-manifest-"));
  const version = "2026.10.1101";
  const expectedNames = [
    "Captures-2026.10.1101-aarch64-apple-darwin.app.tar.gz",
    "Captures-2026.10.1101-x86_64-pc-windows-msvc-setup.exe",
    "Captures-2026.10.1101-x86_64-unknown-linux-gnu.AppImage",
  ];
  assert.deepEqual(
    Object.keys(TARGETS).map((target) => updaterArtifactName(target, version)),
    expectedNames,
  );
  const debName = "Captures-2026.10.1101-x86_64-unknown-linux-gnu.deb";
  for (const name of [...expectedNames, debName]) {
    writeFileSync(join(directory, name), "artifact");
    writeFileSync(
      join(directory, `${name}.sig`),
      Buffer.from(
        `untrusted comment: fixture\nfixture\ntrusted comment: timestamp:1\tfile:${name}\nfixture\n`,
      ).toString("base64"),
    );
  }
  const result = manifest(
    directory,
    version,
    "joswayski/captures",
    "v2026.10.11.1",
    "Native Preview",
  );
  assert.equal(result.notes, "Native Preview");
  assert.deepEqual(Object.keys(result.platforms), [
    "darwin-aarch64",
    "windows-x86_64",
    "linux-x86_64",
    "linux-x86_64-deb",
  ]);
  assert.equal(
    result.platforms["windows-x86_64"].url,
    "https://github.com/joswayski/captures/releases/download/v2026.10.11.1/Captures-2026.10.1101-x86_64-pc-windows-msvc-setup.exe",
  );
  assert.equal(
    result.platforms["linux-x86_64-deb"].url,
    "https://github.com/joswayski/captures/releases/download/v2026.10.11.1/Captures-2026.10.1101-x86_64-unknown-linux-gnu.deb",
  );
  assert.notEqual(
    result.platforms["linux-x86_64-deb"].signature,
    result.platforms["linux-x86_64"].signature,
  );
  assert.equal(result.platforms["linux-x86_64"].size, 8);
  writeFileSync(
    join(directory, `${expectedNames[2]}.sig`),
    Buffer.from("trusted comment: timestamp:1\tfile:other.AppImage\n").toString("base64"),
  );
  assert.throws(
    () => manifest(directory, version, "joswayski/captures", "v2026.10.11.1"),
    /does not bind/u,
  );
});
