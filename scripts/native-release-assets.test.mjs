import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
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
