import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { PREVIEW_CHANNEL_ASSET_NAMES } from "./preview-release-assets.mjs";
import { NATIVE_ASSETS } from "./native-release-assets.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function read(path) {
  return readFileSync(join(root, path), "utf8");
}

const INSTALLER_NAMES = Object.entries(PREVIEW_CHANNEL_ASSET_NAMES)
  .filter(([platform]) => platform !== "updater")
  .map(([, name]) => name);

test("published builds keep a signed in-app updater and a public installer page", () => {
  const config = JSON.parse(read("apps/desktop/src-tauri/tauri.conf.json"));
  assert.equal(config.bundle.createUpdaterArtifacts, true);
  assert.equal(
    typeof config.plugins.updater.pubkey,
    "string",
  );
  assert.ok(
    config.plugins.updater.pubkey.length > 80,
    "installed copies cannot verify a later Preview without the updater public key",
  );
  assert.deepEqual(config.plugins.updater.endpoints, [
    "https://captur.es/api/updates/preview",
    "https://github.com/joswayski/captures/releases/download/preview/latest.json",
  ]);

  const updates = read("apps/desktop/src-tauri/src/updates.rs");
  assert.match(
    updates,
    /const DOWNLOAD_PAGE_URL: &str = "https:\/\/captur\.es\/#download";/u,
  );

  const preferences = read("apps/desktop/ui/src/App.tsx");
  assert.match(preferences, /source="preferences"/u);
  assert.match(preferences, /open_update_download_page/u);
});

test("default downloads use native packages while Tauri recovery stays available", () => {
  const readme = read("README.md");
  const home = read("apps/web/src/pages/Home.tsx");
  const releases = read("docs/releases.md");

  for (const name of NATIVE_ASSETS) {
    const url = `https://captur.es/download/preview/${name}`;
    assert.ok(readme.includes(url), `README is missing ${url}`);
    assert.ok(home.includes(name), `website is missing ${name}`);
    assert.ok(releases.includes(name), `docs/releases.md is missing ${name}`);
  }

  for (const name of INSTALLER_NAMES) assert.ok(releases.includes(name));
  assert.match(readme, /releases\/tag\/preview/u);
  assert.match(home, /const PREVIEW_DOWNLOAD_BASE = "\/download\/preview"/u);
  assert.match(home, /Download Captures\{" "\}/u);
  assert.doesNotMatch(home, /The gallery below shows|Download Captures Native Preview/u);
  assert.match(readme, /Updates are manual/u);
  assert.match(releases, /~\/\.local\/bin\/Captures\.AppImage/u);
});
