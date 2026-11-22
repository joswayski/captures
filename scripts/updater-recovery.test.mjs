import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "vitest";
import { fileURLToPath } from "node:url";

import { PREVIEW_CHANNEL_ASSET_NAMES } from "./preview-release-assets.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function read(path) {
  return readFileSync(join(root, path), "utf8");
}

const INSTALLER_NAMES = Object.entries(PREVIEW_CHANNEL_ASSET_NAMES)
  .filter(([platform]) => platform !== "updater")
  .map(([, name]) => name);

test("published builds keep a signed in-app updater", () => {
  const config = JSON.parse(read("apps/native/bundle/tauri.conf.json"));
  assert.equal(config.bundle.createUpdaterArtifacts, true);
  assert.equal(typeof config.plugins.updater.pubkey, "string");
  assert.ok(
    config.plugins.updater.pubkey.length > 80,
    "installed copies cannot verify a later release without the updater public key",
  );
});

test("README, website, and Preview channel share stable installer names", () => {
  const readme = read("README.md");
  const home = read("apps/web/src/pages/Home.tsx");
  const releases = read("docs/releases.md");

  for (const name of INSTALLER_NAMES) {
    const url = `https://github.com/joswayski/captures/releases/download/preview/${name}`;
    assert.ok(readme.includes(url), `README is missing ${url}`);
    assert.ok(home.includes(name), `website is missing ${name}`);
    assert.ok(releases.includes(name), `docs/releases.md is missing ${name}`);
  }

  assert.match(readme, /If Captures will not open or cannot install an update/u);
  assert.match(readme, /~\/\.local\/bin\/Captures\.AppImage/u);
  assert.doesNotMatch(home, /Installing (?:a download|a Debian package) replaces/u);
  // The homepage leaves AppImage recovery to the README and the app's update error.
  assert.match(releases, /~\/\.local\/bin\/Captures\.AppImage/u);
});
