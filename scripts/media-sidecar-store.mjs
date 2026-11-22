// A second home for the unsigned FFmpeg sidecars that
// .github/actions/prepare-media caches: one archive per cache key on a draft
// GitHub release. GitHub evicts the least recently used Actions caches once a
// repository holds 10 GB, and a cache only release packaging reads is always
// the least recently used, so every release recompiled FFmpeg (3 to 7 minutes
// per platform). Release assets are never evicted.
//
//   media-sidecar-store.mjs ensure
//     Find or create the draft release; writes `release_id` to GITHUB_OUTPUT.
//   media-sidecar-store.mjs restore RELEASE_ID KEY
//     Unpack KEY's archive into the working tree; writes `hit` to GITHUB_OUTPUT.
//   media-sidecar-store.mjs save RELEASE_ID KEY
//     Upload KEY's archive unless it is stored, then drop the platform's older keys.
//
// The store only saves time: every failure is a warning, and packaging then
// builds FFmpeg from source as it does on a cache miss. Restored binaries get
// the same validation as cached ones. Needs GH_TOKEN with contents write (draft
// releases are invisible to read-only tokens), GITHUB_REPOSITORY and `tar`.
import { execFileSync } from "node:child_process";
import { appendFileSync, createWriteStream, globSync, mkdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { pathToFileURL } from "node:url";
import {
  apiUrl,
  deleteAsset,
  githubRequest,
  releaseAssets,
  uploadReleaseAssets,
} from "./github-release-assets.mjs";

// A draft has no public page and no Git tag, and release selection ignores drafts.
export const STORE_NAME = "Build cache (not a release)";
const STORE_TAG = "build-cache";
const STORE_BODY =
  "Unsigned FFmpeg sidecar builds reused by release packaging (scripts/media-sidecar-store.mjs). Not a Captures release; deleting it only makes the next release rebuild FFmpeg.";

// The files prepare-media caches; a test keeps this equal to the action's list.
export const SIDECAR_PATTERNS = [
  "apps/desktop/src-tauri/binaries/ffmpeg-*",
  "apps/desktop/src-tauri/binaries/ffprobe-*",
  "apps/desktop/src-tauri/ffmpeg/COPYING.LGPLv2.1",
  "target/ffmpeg-build/*.tar.xz",
  "target/ffmpeg-build/*.tar.xz.asc",
  "target/ffmpeg-dist",
];

// ffmpeg-v1-<os>-<arch>-<toolchain digest>-<source hash>, from prepare-media.
const KEY = /^(ffmpeg-v\d+-[A-Za-z0-9]+-[A-Za-z0-9]+-)[0-9a-f]+-[0-9a-f]+$/u;
const SCRATCH = "target/media-sidecar-store";

function assetName(key) {
  if (!KEY.test(key)) throw new Error(`unexpected media cache key: ${key}`);
  return `${key}.tar.gz`;
}

export async function ensureStore() {
  const stores = [];
  for (let page = 1; ; page += 1) {
    const releases = await (
      await githubRequest(apiUrl(`releases?per_page=100&page=${page}`))
    ).json();
    stores.push(...releases.filter((release) => release.draft && release.name === STORE_NAME));
    if (releases.length < 100) break;
  }
  // GitHub's release list can lag a create by a few seconds, so two runs that
  // close together could each create a store. The oldest one always wins.
  if (stores.length > 0) return Math.min(...stores.map((store) => store.id));
  const created = await githubRequest(apiUrl("releases"), {
    method: "POST",
    body: JSON.stringify({
      tag_name: STORE_TAG,
      name: STORE_NAME,
      body: STORE_BODY,
      draft: true,
    }),
  });
  return (await created.json()).id;
}

// Paths stay relative to `root`: GNU tar reads a Windows drive letter as a host.
function tar(root, args) {
  execFileSync("tar", args, { cwd: root, stdio: "inherit" });
}

export async function restoreSidecars(releaseId, key, root = process.cwd()) {
  const name = assetName(key);
  const asset = (await releaseAssets(releaseId)).find((candidate) => candidate.name === name);
  if (!asset) return false;
  mkdirSync(join(root, SCRATCH), { recursive: true });
  const archive = `${SCRATCH}/${name}`;
  try {
    const response = await githubRequest(apiUrl(`releases/assets/${asset.id}`), {
      headers: { Accept: "application/octet-stream" },
    });
    await pipeline(Readable.fromWeb(response.body), createWriteStream(join(root, archive)));
    tar(root, ["-xzf", archive]);
  } catch (error) {
    // Leave no half-unpacked binary for the build to mistake for a finished one.
    for (const path of globSync(SIDECAR_PATTERNS.slice(0, 2), { cwd: root }))
      rmSync(join(root, path), { force: true });
    throw error;
  } finally {
    rmSync(join(root, SCRATCH), { recursive: true, force: true });
  }
  return true;
}

export async function saveSidecars(releaseId, key, root = process.cwd()) {
  const name = assetName(key);
  const stored = await releaseAssets(releaseId);
  if (stored.some((asset) => asset.name === name)) return false;
  const files = SIDECAR_PATTERNS.flatMap((pattern) => globSync(pattern, { cwd: root }))
    .map((path) => path.replaceAll("\\", "/"))
    .sort();
  for (const tool of ["ffmpeg", "ffprobe"]) {
    if (!files.some((path) => path.includes(`/binaries/${tool}-`)))
      throw new Error(`no ${tool} sidecar to store`);
  }
  mkdirSync(join(root, SCRATCH), { recursive: true });
  const archive = `${SCRATCH}/${name}`;
  try {
    tar(root, ["-czf", archive, ...files]);
    await uploadReleaseAssets(releaseId, [join(root, archive)]);
  } finally {
    rmSync(join(root, SCRATCH), { recursive: true, force: true });
  }
  // The key changes with the runner image and compiler; keep one build per platform.
  const platform = key.match(KEY)[1];
  for (const asset of stored) {
    if (asset.name.startsWith(platform)) await deleteAsset(asset.id);
  }
  return true;
}

function output(name, value) {
  process.stdout.write(`${name}=${value}\n`);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${value}\n`);
}

async function main([command, releaseId, key]) {
  const stores = /^[1-9]\d*$/u.test(releaseId ?? "") && key;
  try {
    if (command === "ensure") output("release_id", await ensureStore());
    else if (command === "restore" && stores) output("hit", await restoreSidecars(releaseId, key));
    else if (command === "save" && stores) {
      const saved = await saveSidecars(releaseId, key);
      process.stdout.write(saved ? `Stored ${key}.\n` : `${key} is already stored.\n`);
    } else {
      process.stderr.write(
        "usage: media-sidecar-store.mjs ensure | restore RELEASE_ID KEY | save RELEASE_ID KEY\n",
      );
      process.exitCode = 2;
    }
  } catch (error) {
    process.stdout.write(`::warning::Media sidecar store ${command} failed: ${error.message}\n`);
    if (command === "restore") output("hit", false);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main(process.argv.slice(2));
}
