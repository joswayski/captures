import { readdirSync, renameSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const REQUIRED_ASSETS = {
  macos: (name) => name.endsWith(".dmg"),
  windows: (name) => name.endsWith("-setup.exe") || name.endsWith("_setup.exe"),
  linuxDeb: (name) => name.endsWith(".deb"),
  linuxAppImage: (name) => name.endsWith(".AppImage"),
  updater: (name) => name === "latest.json",
};

/** Stable filenames on the permanent `preview` channel so README links never go stale. */
export const PREVIEW_CHANNEL_ASSET_NAMES = {
  macos: "Captures-macOS-Apple-Silicon.dmg",
  windows: "Captures-Windows-x64-setup.exe",
  linuxDeb: "Captures-Linux-x64.deb",
  linuxAppImage: "Captures-Linux-x64.AppImage",
  updater: "latest.json",
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

/**
 * Validate installer assets in `directory`, then rename them to the stable
 * Preview channel names used by the README and permanent `preview` release.
 * Accepts either versioned Tauri package names or names that are already stable.
 */
export function preparePreviewChannelAssets(directory) {
  const root = resolve(directory);
  const selected = previewReleaseAssetsIn(root);
  const prepared = {};

  for (const [platform, sourceName] of Object.entries(selected)) {
    const targetName = PREVIEW_CHANNEL_ASSET_NAMES[platform];
    if (!targetName) {
      throw new Error(`No stable Preview channel name for ${platform}`);
    }
    if (sourceName !== targetName) {
      renameSync(join(root, sourceName), join(root, targetName));
    }
    prepared[platform] = targetName;
  }

  return prepared;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [command, directory] = process.argv.slice(2);
  if (!directory || (command !== "select" && command !== "prepare")) {
    throw new Error(
      "usage: node scripts/preview-release-assets.mjs <select|prepare> <directory>",
    );
  }
  const result =
    command === "prepare"
      ? preparePreviewChannelAssets(directory)
      : previewReleaseAssetsIn(directory);
  process.stdout.write(`${JSON.stringify(result)}\n`);
}
