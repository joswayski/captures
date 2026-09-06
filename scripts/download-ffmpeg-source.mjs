import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";

export const DEFAULT_REPOSITORY = "joswayski/captures";
export const DEFAULT_TIMEOUT_MS = 20_000;
export const DEFAULT_ATTEMPTS_PER_URL = 2;
export const USER_AGENT = "captures-ffmpeg-source";

export function pinnedFfmpegFromScript(source) {
  const version = /^FFMPEG_VERSION="([^"]+)"/mu.exec(source)?.[1];
  const sha256 = /^FFMPEG_SHA256="([^"]+)"/mu.exec(source)?.[1];
  if (!version || !/^[0-9a-f]{64}$/u.test(sha256 ?? "")) {
    throw new Error("scripts/build-ffmpeg-sidecars.sh must pin FFMPEG_VERSION and FFMPEG_SHA256");
  }
  return {
    version,
    sha256,
    archiveName: `ffmpeg-${version}.tar.xz`,
    signatureName: `ffmpeg-${version}.tar.xz.asc`,
  };
}

export function unique(values) {
  return [...new Set(values.filter(Boolean))];
}

export function ffmpegSourceUrls({
  filename,
  githubUrls = [],
  githubActions = false,
}) {
  const canonical = [
    `https://ffmpeg.org/releases/${filename}`,
    `https://www.ffmpeg.org/releases/${filename}`,
  ];
  const ordered = githubActions
    ? [...githubUrls, ...canonical]
    : [...canonical, ...githubUrls];
  return unique(ordered);
}

export function publishedAssetUrls(releases, names) {
  const urls = Object.fromEntries(names.map((name) => [name, []]));
  for (const release of releases) {
    if (release?.draft) continue;
    for (const asset of release?.assets ?? []) {
      if (names.includes(asset?.name) && asset?.browser_download_url) {
        urls[asset.name].push(asset.browser_download_url);
      }
    }
  }
  for (const name of names) {
    urls[name] = unique(urls[name]);
  }
  return urls;
}

export function fileSha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function emptyAssetMap(names) {
  return Object.fromEntries(names.map((name) => [name, []]));
}

export async function fetchPublishedAssetUrls({
  repository,
  names,
  fetchImpl = fetch,
  token,
  apiBase = "https://api.github.com",
  timeoutMs = 15_000,
}) {
  const headers = {
    Accept: "application/vnd.github+json",
    "User-Agent": USER_AGENT,
    "X-GitHub-Api-Version": "2022-11-28",
  };
  if (token) headers.Authorization = `Bearer ${token}`;
  try {
    const response = await fetchImpl(`${apiBase}/repos/${repository}/releases?per_page=100`, {
      headers,
      signal: AbortSignal.timeout(timeoutMs),
    });
    if (!response.ok) return emptyAssetMap(names);
    const releases = await response.json();
    if (!Array.isArray(releases)) return emptyAssetMap(names);
    return publishedAssetUrls(releases, names);
  } catch {
    return emptyAssetMap(names);
  }
}

async function downloadUrl(url, dest, fetchImpl, timeoutMs) {
  const response = await fetchImpl(url, {
    headers: { "User-Agent": USER_AGENT },
    redirect: "follow",
    signal: AbortSignal.timeout(timeoutMs),
  });
  if (!response.ok) {
    throw new Error(`${url} returned HTTP ${response.status}`);
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  mkdirSync(dirname(dest), { recursive: true });
  const temporary = `${dest}.partial`;
  writeFileSync(temporary, bytes);
  rmSync(dest, { force: true });
  renameSync(temporary, dest);
}

export async function downloadPinnedFile({
  dest,
  urls,
  sha256,
  fetchImpl = fetch,
  timeoutMs = DEFAULT_TIMEOUT_MS,
  attemptsPerUrl = DEFAULT_ATTEMPTS_PER_URL,
  log = (message) => process.stdout.write(`${message}\n`),
}) {
  if (urls.length === 0) {
    throw new Error(`No download URLs were provided for ${dest}`);
  }
  if (existsSync(dest) && (!sha256 || fileSha256(dest) === sha256)) {
    return { url: null, reused: true };
  }
  rmSync(dest, { force: true });
  const errors = [];
  for (const url of urls) {
    for (let attempt = 1; attempt <= attemptsPerUrl; attempt++) {
      try {
        log(`Downloading ${url} (attempt ${attempt}/${attemptsPerUrl})`);
        await downloadUrl(url, dest, fetchImpl, timeoutMs);
        if (sha256) {
          const actual = fileSha256(dest);
          if (actual !== sha256) {
            rmSync(dest, { force: true });
            errors.push(`${url}: checksum ${actual}, expected ${sha256}`);
            break;
          }
        }
        log(`Saved ${dest} from ${url}`);
        return { url, reused: false };
      } catch (error) {
        rmSync(dest, { force: true });
        rmSync(`${dest}.partial`, { force: true });
        errors.push(`${url} attempt ${attempt}: ${error.message}`);
      }
    }
  }
  throw new Error(`Failed to download ${dest}:\n${errors.join("\n")}`);
}

export function sourcePaths(root, pin, buildRoot = process.env.CAPTURES_FFMPEG_BUILD_ROOT) {
  const directory = buildRoot || join(root, "target/ffmpeg-build");
  return {
    directory,
    archive: join(directory, pin.archiveName),
    signature: join(directory, pin.signatureName),
  };
}

export async function ensurePinnedFfmpegSource({
  root,
  fetchImpl = fetch,
  env = process.env,
  log = (message) => process.stdout.write(`${message}\n`),
} = {}) {
  const workspace = root || env.GITHUB_WORKSPACE || process.cwd();
  const pin = pinnedFfmpegFromScript(
    readFileSync(join(workspace, "scripts/build-ffmpeg-sidecars.sh"), "utf8"),
  );
  const paths = sourcePaths(workspace, pin);
  const repository = env.GITHUB_REPOSITORY || DEFAULT_REPOSITORY;
  const githubAssets = await fetchPublishedAssetUrls({
    repository,
    names: [pin.archiveName, pin.signatureName],
    fetchImpl,
    token: env.GITHUB_TOKEN || env.GH_TOKEN,
  });
  const githubActions = env.GITHUB_ACTIONS === "true";
  await downloadPinnedFile({
    dest: paths.archive,
    urls: ffmpegSourceUrls({
      filename: pin.archiveName,
      githubUrls: githubAssets[pin.archiveName],
      githubActions,
    }),
    sha256: pin.sha256,
    fetchImpl,
    log,
  });
  await downloadPinnedFile({
    dest: paths.signature,
    urls: ffmpegSourceUrls({
      filename: pin.signatureName,
      githubUrls: githubAssets[pin.signatureName],
      githubActions,
    }),
    fetchImpl,
    log,
  });
  return { pin, paths };
}

function parseArgs(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--root") {
      options.root = argv[i + 1];
      i += 1;
      if (!options.root) throw new Error("usage: download-ffmpeg-source.mjs [--root <dir>]");
    } else {
      throw new Error(`unexpected argument: ${argv[i]}`);
    }
  }
  return options;
}

async function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  const result = await ensurePinnedFfmpegSource({ root: options.root });
  process.stdout.write(
    `Pinned FFmpeg ${result.pin.version} source is ready in ${result.paths.directory}.\n`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
