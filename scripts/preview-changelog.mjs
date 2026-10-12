import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

import { comparePreviewVersions, encodedPreviewVersion } from "./preview-release.mjs";

export function previewChangelog(releases, throughTag) {
  const through = throughTag === undefined ? null : encodedPreviewVersion(throughTag);
  if (throughTag !== undefined && !through) throw new Error("invalid Preview changelog tag");
  return releases
    .flatMap((release) => {
      if (release?.draft || !release?.prerelease) return [];
      const encoded = encodedPreviewVersion(release.tag_name);
      if (!encoded) return [];
      if (through && comparePreviewVersions(encoded.order, through.order) > 0) return [];
      const notes = typeof release.body === "string" ? release.body : "";
      return [
        {
          version: encoded.version,
          display_version: encoded.displayVersion,
          notes,
          order: encoded.order,
        },
      ];
    })
    .sort((left, right) => comparePreviewVersions(right.order, left.order))
    .map((entry) => ({
      version: entry.version,
      display_version: entry.display_version,
      notes: entry.notes,
    }));
}

export function applyPreviewChangelog(latest, releases, throughTag) {
  const changelog = previewChangelog(releases, throughTag);
  if (changelog.length === 0) {
    throw new Error("Preview changelog is empty");
  }
  if (latest?.version !== changelog[0].version) {
    throw new Error(
      `latest.json version ${latest?.version} does not match newest changelog ${changelog[0].version}`,
    );
  }
  return { ...latest, changelog };
}

function main() {
  const [command, latestPath, releasesPath, throughTag] = process.argv.slice(2);
  if (command !== "apply" || !latestPath || !releasesPath) {
    throw new Error(
      "usage: node scripts/preview-changelog.mjs apply <latest.json> <releases.json> [through-tag]",
    );
  }
  const latest = JSON.parse(readFileSync(latestPath, "utf8"));
  const releases = JSON.parse(readFileSync(releasesPath, "utf8"));
  const updated = applyPreviewChangelog(latest, releases, throughTag);
  writeFileSync(latestPath, `${JSON.stringify(updated, null, 2)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
