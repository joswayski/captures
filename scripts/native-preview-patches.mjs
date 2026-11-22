// Patch updates for the public and staging feeds (format and client:
// crates/captures-app/src/updater/preview/delta.rs).
//
// `generate TARGET VERSION DIRECTORY [--staging]` runs after `collect` in a
// package job, with the updater signing key and GH_TOKEN. For the macOS and
// AppImage updater artifacts in DIRECTORY it writes
// `Captures-<version>-<target>.from-<base>.patch` from each of the BASES newest
// earlier native builds of the same feed that still carry their artifact, and
// for macOS the signature of the bundle tar those patches rebuild. Patches not
// smaller than half the full artifact are dropped. A failure is a warning,
// never a failed release: clients then download the full artifact.
// `manifest` in native-preview-build.mjs lists what it wrote.
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readdirSync, rmSync, statSync, unlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  TARGETS,
  patchName,
  patchTargetName,
  updaterArtifactName,
} from "./native-preview-build.mjs";

const ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));
export const BASES = 5;
const NATIVE_MARKER = "captures-native.json";
const PUBLIC_TAG = /^v\d{4}\.\d{2}\.\d{2}\.[1-9]\d?$/u;
const STAGING_TAG = /^native-staging-\d+\.\d+\.\d+$/u;

export function patchable(target) {
  return target === "aarch64-apple-darwin" || target === "x86_64-unknown-linux-gnu";
}

function compareVersions(left, right) {
  const a = left.split(".").map(Number);
  const b = right.split(".").map(Number);
  for (let index = 0; index < 3; index += 1) if (a[index] !== b[index]) return a[index] - b[index];
  return 0;
}

/**
 * Earlier native builds of one feed that installed apps may run, newest first:
 * staging prereleases, or public releases carrying the native runtime marker
 * (Tauri installs cannot apply patches).
 */
export function baseReleases(releases, target, version, staging) {
  const suffix = TARGETS[target].suffix;
  const pattern = new RegExp(
    `^Captures-(\\d+\\.\\d+\\.\\d+)-${target.replaceAll(".", "\\.")}${suffix.replaceAll(".", "\\.")}$`,
    "u",
  );
  const uploaded = (release, name) =>
    release.assets?.some((asset) => asset.name === name && asset.state === "uploaded");
  return releases
    .filter((release) => !release.draft)
    .filter((release) =>
      staging
        ? STAGING_TAG.test(release.tag_name)
        : PUBLIC_TAG.test(release.tag_name) && uploaded(release, NATIVE_MARKER),
    )
    .flatMap((release) =>
      (release.assets ?? [])
        .filter((asset) => asset.state === "uploaded")
        .flatMap((asset) => {
          const match = pattern.exec(asset.name);
          return match ? [{ tag: release.tag_name, name: asset.name, version: match[1] }] : [];
        }),
    )
    .filter((base) => compareVersions(base.version, version) < 0)
    .sort((left, right) => compareVersions(right.version, left.version))
    .filter((base, index, all) => index === 0 || all[index - 1].version !== base.version)
    .slice(0, BASES);
}

function run(command, args, options = {}) {
  return execFileSync(command, args, { stdio: ["ignore", "pipe", "inherit"], ...options });
}

/** The file a patch for this target starts from or rebuilds, from an artifact. */
function patchInput(target, artifact, work, helper) {
  if (!target.includes("apple-darwin")) return artifact;
  const unpacked = mkdtempSync(join(work, "app-"));
  run("tar", ["-xzf", artifact, "-C", unpacked]);
  const [app, ...others] = readdirSync(unpacked);
  if (!app?.endsWith(".app") || others.length) throw new Error(`${artifact} is not one app bundle`);
  const tar = join(work, `${app}-${readdirSync(work).length}.tar`);
  run(helper, ["--bundle-tar", join(unpacked, app), tar]);
  rmSync(unpacked, { recursive: true, force: true });
  return tar;
}

export function generate(target, version, directory, staging) {
  if (!patchable(target)) return;
  const suffix = target.includes("windows") ? ".exe" : "";
  const helper = join(ROOT, "target", target, "release", `native_preview_update_helper${suffix}`);
  const artifact = join(directory, updaterArtifactName(target, version));
  const fullSize = statSync(artifact).size;
  const work = mkdtempSync(join(process.env.RUNNER_TEMP ?? tmpdir(), "captures-patches-"));
  try {
    let next = patchInput(target, artifact, work, helper);
    if (target.includes("apple-darwin")) {
      const named = join(work, patchTargetName(target, version));
      copyFileSync(next, named);
      next = named;
      run(process.execPath, [
        join(ROOT, "node_modules/@tauri-apps/cli/tauri.js"),
        "signer",
        "sign",
        next,
      ]);
      run(helper, ["--verify", version, next, `${next}.sig`]);
      copyFileSync(`${next}.sig`, join(directory, `${patchTargetName(target, version)}.sig`));
    }
    const releases = JSON.parse(
      run(
        "gh",
        [
          "api",
          "--paginate",
          "--slurp",
          `repos/${process.env.GITHUB_REPOSITORY}/releases?per_page=100`,
        ],
        {
          maxBuffer: 256 * 1024 * 1024,
        },
      ),
    ).flat();
    const bases = baseReleases(releases, target, version, staging);
    if (!bases.length) console.log(`No earlier ${target} builds to patch from.`);
    for (const base of bases) {
      const output = join(directory, patchName(target, version, base.version));
      try {
        const downloads = mkdtempSync(join(work, "base-"));
        run("gh", ["release", "download", base.tag, "--pattern", base.name, "--dir", downloads]);
        const from = patchInput(target, join(downloads, base.name), work, helper);
        run(helper, ["--make-patch", from, next, output]);
        rmSync(downloads, { recursive: true, force: true });
        if (from !== join(downloads, base.name)) rmSync(from, { force: true });
        const size = statSync(output).size;
        if (size * 2 >= fullSize) {
          unlinkSync(output);
          console.log(`Dropped the ${base.version} patch: ${size} of ${fullSize} bytes.`);
        } else {
          console.log(`Patch from ${base.version}: ${size} of ${fullSize} bytes.`);
        }
      } catch (error) {
        rmSync(output, { force: true });
        console.log(`::warning::No ${target} patch from ${base.version}: ${error.message}`);
      }
    }
  } catch (error) {
    for (const name of readdirSync(directory)) {
      if (name.endsWith(".patch") || name.endsWith(".app.tar.sig"))
        rmSync(join(directory, name), { force: true });
    }
    console.log(
      `::warning::No ${target} patch updates; clients download in full: ${error.message}`,
    );
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const [operation, target, version, directory, ...flags] = process.argv.slice(2);
  if (
    operation !== "generate" ||
    !TARGETS[target] ||
    !directory ||
    flags.some((flag) => flag !== "--staging")
  )
    throw new Error(
      "usage: native-preview-patches.mjs generate TARGET VERSION DIRECTORY [--staging]",
    );
  generate(target, version, resolve(directory), flags.includes("--staging"));
}
