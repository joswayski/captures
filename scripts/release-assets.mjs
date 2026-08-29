import { createHash } from "node:crypto";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const PLATFORM_KEYS = ["darwin-aarch64", "windows-x86_64", "linux-x86_64"];
const GITHUB_API_ASSET_URL =
  /^https:\/\/api\.github\.com\/repos\/[^/]+\/[^/]+\/releases\/assets\/(\d+)$/u;

export function isGitHubApiAssetUrl(url) {
  return GITHUB_API_ASSET_URL.test(String(url ?? ""));
}

/**
 * Tauri's GitHub Action writes unauthenticated GitHub API asset URLs into
 * `latest.json`. Those downloads count against the 60-request/hour API budget
 * and often fail with 403. Rewrite them to the public Releases download URLs.
 */
export function rewriteGithubApiAssetUrls(latest, assets) {
  const urlsById = new Map(
    (assets ?? [])
      .filter((asset) => asset?.id != null && asset.browser_download_url)
      .map((asset) => [String(asset.id), String(asset.browser_download_url)]),
  );
  const platforms = latest?.platforms ?? {};
  let rewritten = 0;
  for (const [platform, entry] of Object.entries(platforms)) {
    const match = String(entry?.url ?? "").match(GITHUB_API_ASSET_URL);
    if (!match) continue;
    const next = urlsById.get(match[1]);
    if (!next) {
      throw new Error(`no Releases download URL for ${platform} asset ${match[1]}`);
    }
    entry.url = next;
    rewritten += 1;
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
