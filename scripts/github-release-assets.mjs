import { createReadStream, createWriteStream } from "node:fs";
import { mkdir, readdir, stat } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { pathToFileURL } from "node:url";

const API_VERSION = "2022-11-28";

function configuration() {
  const repository = process.env.GITHUB_REPOSITORY;
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  if (!repository || !/^[^/\s]+\/[^/\s]+$/u.test(repository)) {
    throw new Error("GITHUB_REPOSITORY must identify the repository as owner/name");
  }
  if (!token) throw new Error("GH_TOKEN or GITHUB_TOKEN is required");
  return { repository, token };
}

async function githubRequest(url, options = {}) {
  const { token } = configuration();
  const response = await fetch(url, {
    ...options,
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "X-GitHub-Api-Version": API_VERSION,
      ...options.headers,
    },
  });
  if (!response.ok) {
    const detail = (await response.text()).trim();
    throw new Error(`GitHub request failed (${response.status}): ${detail || response.statusText}`);
  }
  return response;
}

function apiUrl(path) {
  const { repository } = configuration();
  return `https://api.github.com/repos/${repository}/${path}`;
}

async function release(releaseId) {
  const response = await githubRequest(apiUrl(`releases/${releaseId}`));
  return response.json();
}

async function releaseAssets(releaseId) {
  const response = await githubRequest(apiUrl(`releases/${releaseId}/assets?per_page=100`));
  return response.json();
}

async function deleteAsset(assetId) {
  await githubRequest(apiUrl(`releases/assets/${assetId}`), { method: "DELETE" });
}

export async function uploadReleaseAssets(releaseId, paths) {
  if (paths.length === 0) throw new Error("at least one release asset is required");
  const currentRelease = await release(releaseId);
  const uploadUrl = currentRelease.upload_url?.replace(/\{\?.*$/u, "");
  if (!uploadUrl) throw new Error(`release ${releaseId} is missing its upload URL`);

  for (const path of paths) {
    const name = basename(path);
    const metadata = await stat(path);
    if (!metadata.isFile()) throw new Error(`release asset is not a file: ${path}`);

    const existing = (await releaseAssets(releaseId)).filter((asset) => asset.name === name);
    for (const asset of existing) await deleteAsset(asset.id);

    const body = createReadStream(path);
    await githubRequest(`${uploadUrl}?name=${encodeURIComponent(name)}`, {
      method: "POST",
      headers: {
        "Content-Length": String(metadata.size),
        "Content-Type": "application/octet-stream",
      },
      body,
      duplex: "half",
    });
    process.stdout.write(`Uploaded ${name} to release ${releaseId}.\n`);
  }
}

export async function downloadReleaseAssets(releaseId, directory) {
  const destination = resolve(directory);
  await mkdir(destination, { recursive: true });
  if ((await readdir(destination)).length !== 0) {
    throw new Error(`release asset directory must be empty: ${destination}`);
  }

  const assets = await releaseAssets(releaseId);
  if (assets.length === 0) throw new Error(`release ${releaseId} has no assets`);
  for (const asset of assets) {
    if (!asset.name || basename(asset.name) !== asset.name || /[/\\]/u.test(asset.name)) {
      throw new Error(`release ${releaseId} has an unsafe asset name`);
    }
    const response = await githubRequest(apiUrl(`releases/assets/${asset.id}`), {
      headers: { Accept: "application/octet-stream" },
    });
    if (!response.body) throw new Error(`release asset ${asset.name} returned no body`);
    await pipeline(Readable.fromWeb(response.body), createWriteStream(join(destination, asset.name)));
    process.stdout.write(`Downloaded ${asset.name} from release ${releaseId}.\n`);
  }
}

export async function syncReleaseAssets(releaseId, directory) {
  const source = resolve(directory);
  const entries = await readdir(source, { withFileTypes: true });
  if (entries.length === 0) {
    throw new Error(`release asset directory must not be empty: ${source}`);
  }

  const paths = entries
    .sort((left, right) => left.name.localeCompare(right.name))
    .map((entry) => {
      if (!entry.isFile() || basename(entry.name) !== entry.name || /[/\\]/u.test(entry.name)) {
        throw new Error(`release asset directory contains an unsafe entry: ${entry.name}`);
      }
      return join(source, entry.name);
    });
  const expectedNames = new Set(paths.map((path) => basename(path)));

  // Upload the complete new set before removing versioned assets from the prior
  // Preview. A failed sync therefore leaves at least one usable installer set.
  await uploadReleaseAssets(releaseId, paths);
  for (const asset of await releaseAssets(releaseId)) {
    if (!expectedNames.has(asset.name)) {
      await deleteAsset(asset.id);
      process.stdout.write(`Removed stale ${asset.name} from release ${releaseId}.\n`);
    }
  }
}

async function main(args) {
  const [command, releaseId, ...rest] = args;
  if (!/^[1-9]\d*$/u.test(releaseId ?? "")) {
    throw new Error("release ID must be a positive integer");
  }
  if (command === "upload") {
    await uploadReleaseAssets(releaseId, rest);
    return;
  }
  if (command === "download") {
    if (rest.length !== 1) {
      throw new Error("download requires exactly one destination directory");
    }
    await downloadReleaseAssets(releaseId, rest[0]);
    return;
  }
  if (command === "sync") {
    if (rest.length !== 1) {
      throw new Error("sync requires exactly one source directory");
    }
    await syncReleaseAssets(releaseId, rest[0]);
    return;
  }
  throw new Error(
    "usage: node scripts/github-release-assets.mjs <upload|download|sync> <release-id> <paths...>",
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main(process.argv.slice(2));
}
