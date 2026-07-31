import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const NIGHTLY_TAG = /^v(\d{4})\.(\d{2})\.(\d{2})\.([1-9]\d?)$/u;

export function nightlyVersion(tag) {
  const match = NIGHTLY_TAG.exec(tag ?? "");
  if (!match) return null;
  return match.slice(1).map(Number);
}

function compareNightlyVersions(left, right) {
  for (let index = 0; index < left.length; index += 1) {
    const order = left[index] - right[index];
    if (order !== 0) return order;
  }
  return 0;
}

export function latestNightlyRelease(releases) {
  return releases
    .flatMap((release) => {
      if (release.draft || !release.prerelease) return [];
      const version = nightlyVersion(release.tag_name);
      return version ? [{ release, version }] : [];
    })
    .sort((left, right) => {
      const versionOrder = compareNightlyVersions(right.version, left.version);
      if (versionOrder !== 0) return versionOrder;
      const dateOrder = Date.parse(right.release.published_at ?? right.release.created_at)
        - Date.parse(left.release.published_at ?? left.release.created_at);
      return dateOrder || right.release.id - left.release.id;
    })[0]?.release ?? null;
}

function main() {
  const [releasesPath] = process.argv.slice(2);
  if (!releasesPath) {
    throw new Error("usage: node scripts/nightly-release.mjs <releases.json>");
  }
  const release = latestNightlyRelease(JSON.parse(readFileSync(releasesPath, "utf8")));
  if (!release) throw new Error("no published Captures Nightly was found");
  process.stdout.write(`${release.tag_name}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
