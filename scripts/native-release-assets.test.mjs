import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { NATIVE_ASSETS, validateNativeAssets } from "./native-release-assets.mjs";
import { latestPreviewRelease } from "./preview-release.mjs";

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "native-assets-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const name of NATIVE_ASSETS) writeFileSync(join(root, name), "abc");
  return root;
}

test("native publication requires all four builds and checksums their bytes", (t) => {
  const root = fixture(t);
  const checksums = validateNativeAssets(root);
  assert.equal(checksums.trim().split("\n").length, 4);
  assert.match(checksums, /^ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  Captures-Linux-x64.deb/m);
  rmSync(join(root, NATIVE_ASSETS[0]));
  assert.throws(() => validateNativeAssets(root), /requires exactly/);
});

test("native releases reject updater metadata, legacy AppImages and empty packages", (t) => {
  const root = fixture(t);
  for (const extra of ["latest.json", "Captures-Linux-x64.AppImage"]) {
    writeFileSync(join(root, extra), "legacy");
    assert.throws(() => validateNativeAssets(root), /requires exactly/);
    rmSync(join(root, extra));
  }
  writeFileSync(join(root, NATIVE_ASSETS[1]), "");
  assert.throws(() => validateNativeAssets(root), /Empty native asset/);
});

test("Tauri update selection ignores newer native releases", () => {
  const legacy = { id: 1, tag_name: "v2026.09.13.1", draft: false, prerelease: true };
  const native = { id: 2, tag_name: "native-v2026.09.14.1", draft: false, prerelease: true };
  assert.equal(latestPreviewRelease([native, legacy]), legacy);
});

test("macOS packaging invokes the non-executable build script through Bash", (t) => {
  const root = join(fixture(t), "checkout with spaces");
  const directory = join(root, "experiments/macos-native");
  mkdirSync(directory, { recursive: true });
  writeFileSync(join(directory, "build.sh"), '#!/usr/bin/env bash\nprintf "native build invoked"\n', { mode: 0o644 });
  const script = readFileSync(new URL("./package-native-macos.sh", import.meta.url), "utf8");
  const invocation = script.split("\n").find((line) => line.includes('$root/experiments/macos-native/build.sh"'));
  assert.ok(invocation, "the packager must invoke the native build");
  const result = spawnSync("bash", ["-c", invocation], {
    env: { ...process.env, root },
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "native build invoked");
});
