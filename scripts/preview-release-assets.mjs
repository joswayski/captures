import { readdirSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const REQUIRED_ASSETS = {
  macos: (name) => name.endsWith(".dmg"),
  windows: (name) => name.endsWith("-setup.exe") || name.endsWith("_setup.exe"),
  linuxDeb: (name) => name.endsWith(".deb"),
  linuxAppImage: (name) => name.endsWith(".AppImage"),
  updater: (name) => name === "latest.json",
};

export function previewReleaseAssets(names) {
  const uniqueNames = [...new Set(names)].sort();
  if (uniqueNames.length !== names.length) {
    throw new Error("Preview release assets contain duplicate names");
  }

  const selected = {};
  for (const [platform, matches] of Object.entries(REQUIRED_ASSETS)) {
    const candidates = uniqueNames.filter(matches);
    if (candidates.length !== 1) {
      throw new Error(
        `Preview release requires exactly one ${platform} asset; found ${candidates.length}`,
      );
    }
    selected[platform] = candidates[0];
  }

  const expectedNames = new Set(Object.values(selected));
  const unexpected = uniqueNames.filter((name) => !expectedNames.has(name));
  if (unexpected.length > 0) {
    throw new Error(`Preview release contains unexpected assets: ${unexpected.join(", ")}`);
  }
  return selected;
}

export function previewReleaseAssetsIn(directory) {
  const root = resolve(directory);
  const entries = readdirSync(root, { withFileTypes: true });
  for (const entry of entries) {
    if (!entry.isFile()) {
      throw new Error(`Preview release asset is not a file: ${entry.name}`);
    }
  }
  return previewReleaseAssets(entries.map((entry) => entry.name));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [directory] = process.argv.slice(2);
  if (!directory) {
    throw new Error("usage: node scripts/preview-release-assets.mjs <directory>");
  }
  process.stdout.write(`${JSON.stringify(previewReleaseAssetsIn(directory))}\n`);
}
