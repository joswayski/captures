import { createHash } from "node:crypto";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const PLATFORM_KEYS = ["darwin-aarch64", "windows-x86_64", "linux-x86_64"];
const GITHUB_API_ASSET_URL =
  /^https:\/\/api\.github\.com\/repos\/[^/]+\/[^/]+\/releases\/assets\/(\d+)$/u;
const GITHUB_RELEASE_DOWNLOAD_URL =
  /^https:\/\/github\.com\/([^/]+\/[^/]+)\/releases\/download\/([^/]+)\/([^/?#]+)$/u;

export function isGitHubApiAssetUrl(url) {
  return GITHUB_API_ASSET_URL.test(String(url ?? ""));
}

export function isGitHubUntaggedDownloadUrl(url) {
  const match = String(url ?? "").match(GITHUB_RELEASE_DOWNLOAD_URL);
  return Boolean(match && decodeURIComponent(match[2]).startsWith("untagged-"));
}

/**
 * Public GitHub Releases download URL for a published tag. Draft releases expose
 * `untagged-*` `browser_download_url`s that 404 after the tag is published, so
 * the updater must never persist those.
 */
export function publicGithubReleaseDownloadUrl(repository, tag, assetName) {
  if (!repository || !/^[^/\s]+\/[^/\s]+$/u.test(repository)) {
    throw new Error("repository must identify owner/name");
  }
  if (!tag || /\s/u.test(tag) || tag.includes("/") || tag.startsWith("untagged-")) {
    throw new Error(`release tag is not a public download tag: ${tag}`);
  }
  if (!assetName || /[/\\]/u.test(assetName)) {
    throw new Error(`unsafe release asset name: ${assetName}`);
  }
  return `https://github.com/${repository}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(assetName)}`;
}

function updaterAssetName(url, assetsById) {
  const value = String(url ?? "");
  const apiMatch = value.match(GITHUB_API_ASSET_URL);
  if (apiMatch) {
    const asset = assetsById.get(apiMatch[1]);
    if (!asset?.name) {
      throw new Error(`no Releases download URL for asset ${apiMatch[1]}`);
    }
    return String(asset.name);
  }
  const downloadMatch = value.match(GITHUB_RELEASE_DOWNLOAD_URL);
  if (!downloadMatch) return null;
  return decodeURIComponent(downloadMatch[3]);
}

/**
 * Tauri's GitHub Action writes unauthenticated GitHub API asset URLs into
 * `latest.json`. Those downloads count against the 60-request/hour API budget
 * and often fail with 403. Rewrite them to public `releases/download/<tag>/…`
 * links using the CalVer tag, not the draft's temporary `untagged-*` URL.
 */
export function rewriteGithubApiAssetUrls(latest, assets, options = {}) {
  const repository = options.repository;
  const tag = options.tag;
  if (!repository || !tag) {
    throw new Error("rewriteGithubApiAssetUrls requires repository and tag");
  }

  const assetsById = new Map(
    (assets ?? [])
      .filter((asset) => asset?.id != null)
      .map((asset) => [String(asset.id), asset]),
  );
  const platforms = latest?.platforms ?? {};
  let rewritten = 0;
  for (const [platform, entry] of Object.entries(platforms)) {
    let name;
    try {
      name = updaterAssetName(entry?.url, assetsById);
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      throw new Error(`${detail} for ${platform}`);
    }
    if (!name) continue;
    const next = publicGithubReleaseDownloadUrl(repository, tag, name);
    if (entry.url !== next) {
      entry.url = next;
      rewritten += 1;
    }
  }
  return rewritten;
}

export function validateAndWriteChecksums(directory, appVersion) {
  const root = resolve(directory);
  const names = readdirSync(root).sort();
  const requiredAssets = [
    ["macOS DMG", (name) => name.endsWith(".dmg")],
    ["macOS updater archive", (name) => name.endsWith(".app.tar.gz")],
    ["macOS updater signature", (name) => name.endsWith(".app.tar.gz.sig")],
    ["Windows NSIS installer", (name) => name.endsWith("-setup.exe") || name.endsWith("_setup.exe")],
    ["Windows updater signature", (name) => name.endsWith(".exe.sig")],
    ["Linux AppImage", (name) => name.endsWith(".AppImage")],
    ["Linux updater signature", (name) => name.endsWith(".AppImage.sig")],
    ["Linux Debian package", (name) => name.endsWith(".deb")],
    ["updater manifest", (name) => name === "latest.json"],
  ];
  for (const [label, predicate] of requiredAssets) {
    if (!names.some(predicate)) throw new Error(`release is missing its ${label}`);
  }

  const ffmpegSource = names.find((name) => /^ffmpeg-[\d.]+\.tar\.xz$/u.test(name));
  if (!ffmpegSource) throw new Error("release is missing its FFmpeg source archive");
  const ffmpegPrefix = ffmpegSource.slice(0, -".tar.xz".length);
  for (const [label, suffix] of [
    ["FFmpeg source signature", ".tar.xz.asc"],
    ["FFmpeg build configuration", "-BUILD_CONFIG.txt"],
    ["FFmpeg LGPL license", "-COPYING.LGPLv2.1"],
    ["FFmpeg notice", "-NOTICE.md"],
  ]) {
    if (!names.includes(`${ffmpegPrefix}${suffix}`)) throw new Error(`release is missing its ${label}`);
  }
  const ffmpegConfiguration = readFileSync(join(root, `${ffmpegPrefix}-BUILD_CONFIG.txt`), "utf8");
  for (const flag of ["--disable-gpl", "--disable-nonfree", "--disable-version3"]) {
    if (!ffmpegConfiguration.includes(flag)) throw new Error(`FFmpeg build configuration is missing ${flag}`);
  }
  for (const flag of ["--enable-gpl", "--enable-nonfree", "--enable-version3", "--enable-libx264"]) {
    if (ffmpegConfiguration.includes(flag)) {
      throw new Error(`FFmpeg build configuration contains forbidden flag ${flag}`);
    }
  }

  const latest = JSON.parse(readFileSync(join(root, "latest.json"), "utf8"));
  if (latest.version !== appVersion) {
    throw new Error(`latest.json version ${latest.version} does not match ${appVersion}`);
  }
  if (typeof latest.notes !== "string" || latest.notes.trim() === "") {
    throw new Error("latest.json is missing release notes");
  }
  for (const platform of PLATFORM_KEYS) {
    const entry = latest.platforms?.[platform];
    if (!entry?.url || !entry?.signature) {
      throw new Error(`latest.json is missing a complete ${platform} entry`);
    }
    if (isGitHubApiAssetUrl(entry.url)) {
      throw new Error(
        `latest.json ${platform} still points at the GitHub API (${entry.url}); rewrite it to a Releases download URL before publishing`,
      );
    }
    if (isGitHubUntaggedDownloadUrl(entry.url)) {
      throw new Error(
        `latest.json ${platform} still points at a draft untagged download (${entry.url}); rewrite it to the published tag URL before publishing`,
      );
    }
  }

  const checksumNames = names.filter((name) => name !== "SHA256SUMS");
  const checksums = checksumNames.map((name) => {
    const hash = createHash("sha256").update(readFileSync(join(root, name))).digest("hex");
    return `${hash}  ${name}`;
  });
  writeFileSync(join(root, "SHA256SUMS"), `${checksums.join("\n")}\n`);
  return { names, checksums };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [directory, appVersion] = process.argv.slice(2);
  if (!directory || !appVersion) {
    throw new Error("usage: node scripts/release-assets.mjs <directory> <app-version>");
  }
  const result = validateAndWriteChecksums(directory, appVersion);
  process.stdout.write(`Validated ${result.names.length} release assets.\n`);
}
