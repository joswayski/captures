// Native per-file update patches for a schema-2 native update manifest.
//
// `generate` runs `native_update_patch` from each of the six newest retained
// earlier builds of each target (the `native-update-bases` draft release, named
// `<version>--<target>.<tar.gz|zip>`) to that target's new full archive, keeps
// the patches that are strictly smaller than half the full archive, adds them
// as `delta` descriptors to the manifest's artifacts, and keeps the manifest
// within MAX_MANIFEST_BYTES by dropping the oldest offers. A patch failure is
// only a warning: clients then download the full archive. It never signs; sign
// the exact manifest bytes it writes with the existing signer afterwards.
//
// `stale` reads retained base names on stdin and prints those outside each
// target's newest six builds, for scripts/native-update-retain.sh.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { MAX_MANIFEST_BYTES, manifestText } from "./preview-changelog.mjs";

// Matches crates/captures-app/src/updater/patch.rs.
export const FORMAT = "capturespatch1";
export const MAX_OFFERS = 16;
// Patches from the newest earlier builds kept on the native-update-bases release.
export const RETAINED_BASES = 6;
export const BASES_RELEASE = "native-update-bases";
export const TARGETS = {
  "aarch64-apple-darwin": ".zip",
  "x86_64-apple-darwin": ".zip",
  "x86_64-pc-windows-msvc": ".zip",
  "x86_64-unknown-linux-gnu": ".tar.gz",
};
const GENERATOR_TIMEOUT_MS = 20 * 60 * 1000;
const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;

/** SemVer precedence, as the Rust client compares versions (build metadata ignored). */
export function compareVersions(left, right) {
  const a = SEMVER.exec(left);
  const b = SEMVER.exec(right);
  if (!a || !b) throw new Error(`invalid version ${a ? right : left}`);
  for (let index = 1; index <= 3; index += 1) {
    const difference = BigInt(a[index]) - BigInt(b[index]);
    if (difference !== 0n) return difference < 0n ? -1 : 1;
  }
  if (!a[4] || !b[4]) return a[4] === b[4] ? 0 : a[4] ? -1 : 1;
  const x = a[4].split(".");
  const y = b[4].split(".");
  for (let index = 0; index < Math.min(x.length, y.length); index += 1) {
    if (x[index] === y[index]) continue;
    const numeric = [x[index], y[index]].map((part) => /^\d+$/.test(part));
    if (numeric[0] && numeric[1]) return BigInt(x[index]) < BigInt(y[index]) ? -1 : 1;
    if (numeric[0] !== numeric[1]) return numeric[0] ? -1 : 1;
    return x[index] < y[index] ? -1 : 1;
  }
  return Math.sign(x.length - y.length);
}

/** `<version>--<target><ext>` for a retained full archive, or null. */
export function parseBaseName(name) {
  const match = /^(.+)--([a-z0-9_-]+?)(\.tar\.gz|\.zip)$/.exec(name);
  if (!match || TARGETS[match[2]] !== match[3] || !SEMVER.test(match[1])) return null;
  return { version: match[1], target: match[2] };
}

export function baseName(version, target) {
  if (!SEMVER.test(version) || !TARGETS[target])
    throw new Error(`invalid base ${version} ${target}`);
  return `${version}--${target}${TARGETS[target]}`;
}

export function patchName(target, baseVersion, version) {
  return `captures-native-${target}-${baseVersion}-to-${version}.capturespatch`;
}

/** Whether a patch is worth listing: strictly smaller than half the full artifact. */
export function worthwhile(patchSize, fullSize) {
  return patchSize > 0 && patchSize * 2 < fullSize;
}

/** The newest retained builds of `target` older than `version`, newest first. */
export function retainedBases(names, target, version) {
  return names
    .map((name) => ({ name, ...parseBaseName(name) }))
    .filter((base) => base.target === target && compareVersions(base.version, version) < 0)
    .sort((left, right) => compareVersions(right.version, left.version))
    .slice(0, RETAINED_BASES);
}

/** Retained base names outside each target's newest builds; other names stay. */
export function staleBases(names) {
  const kept = new Set();
  for (const target of Object.keys(TARGETS)) {
    const newest = names
      .map((name) => ({ name, ...parseBaseName(name) }))
      .filter((base) => base.target === target)
      .sort((left, right) => compareVersions(right.version, left.version))
      .slice(0, RETAINED_BASES);
    for (const { name } of newest) kept.add(name);
  }
  return names.filter((name) => parseBaseName(name) && !kept.has(name));
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function warn(message) {
  // A GitHub Actions annotation when run there; plain text otherwise.
  console.log(`::warning::${message.replaceAll("\n", " ")}`);
}

/** Exactly the offer shape `patch::Candidate::select` deserializes. */
function offerFrom(output, { target, baseVersion, version, url, path, artifact }) {
  const descriptor = output?.delta;
  const [offer] = descriptor?.patches ?? [];
  if (
    descriptor?.format !== FORMAT ||
    descriptor.patches.length !== 1 ||
    offer.base_version !== baseVersion ||
    offer.url !== url ||
    output.target?.version !== version
  ) {
    throw new Error("unexpected generator output");
  }
  if (output.target.size !== artifact.size || output.target.sha256 !== artifact.sha256) {
    throw new Error(`the ${target} archive does not match the manifest artifact`);
  }
  const size = statSync(path).size;
  const hash = sha256(path);
  if (offer.size !== size || offer.sha256 !== hash) throw new Error("patch file does not match");
  return { base_version: baseVersion, url, size, sha256: hash };
}

/**
 * Generate patches for every artifact in `manifest` (a parsed schema-2 manifest)
 * and return it with `delta` descriptors. Writes patches to `output` and removes
 * any it does not list. Never throws for a patch failure.
 */
export function generatePatches({ manifest, tool, bases, archives, output, patchBaseUrl }) {
  const updated = structuredClone(manifest);
  if (updated.schema !== 2) {
    warn("Only schema-2 manifests list per-file patches; clients download full archives.");
    return updated;
  }
  mkdirSync(output, { recursive: true });
  const names = existsSync(bases) ? readdirSync(bases) : [];
  if (names.length === 0) warn("No retained builds to patch from; clients download full archives.");
  for (const [target, artifact] of Object.entries(updated.artifacts ?? {})) {
    delete artifact.delta;
    if (!TARGETS[target]) {
      warn(`No patches for unknown target ${target}.`);
      continue;
    }
    const offers = [];
    for (const base of retainedBases(names, target, updated.version)) {
      const file = patchName(target, base.version, updated.version);
      const path = join(output, file);
      const url = `${patchBaseUrl.replace(/\/+$/, "")}/${encodeURIComponent(file)}`;
      try {
        rmSync(path, { force: true });
        const archive = join(archives, basename(new URL(artifact.url).pathname));
        // The generator proves each patch by rebuilding the target from the base
        // exactly as a client does before writing it.
        const result = spawnSync(
          tool,
          [
            "--target",
            target,
            "--base",
            resolve(bases, base.name),
            "--base-version",
            base.version,
            "--target-archive",
            resolve(archive),
            "--version",
            updated.version,
            "--patch-url",
            url,
            "--output",
            resolve(path),
          ],
          { encoding: "utf8", maxBuffer: 16 * 1024 * 1024, timeout: GENERATOR_TIMEOUT_MS },
        );
        if (result.error) throw result.error;
        if (result.status !== 0) throw new Error(result.stderr.trim() || `exit ${result.status}`);
        const offer = offerFrom(JSON.parse(result.stdout), {
          target,
          baseVersion: base.version,
          version: updated.version,
          url,
          path,
          artifact,
        });
        if (!worthwhile(offer.size, artifact.size)) {
          console.log(
            `Skipped the ${target} patch from ${base.version}: ${offer.size} of ${artifact.size} bytes is not small enough.`,
          );
          rmSync(path, { force: true });
          continue;
        }
        console.log(
          `${target} patch from ${base.version}: ${offer.size} of ${artifact.size} bytes.`,
        );
        offers.push(offer);
      } catch (error) {
        warn(`No ${target} patch from ${base.version}: ${error.message}`);
        rmSync(path, { force: true });
      }
    }
    if (offers.length > 0) {
      artifact.delta = { format: FORMAT, patches: offers.slice(0, MAX_OFFERS) };
    }
  }
  return fit(updated, output);
}

/** Drop the oldest offers (by base version, across targets) until the manifest fits. */
export function fit(manifest, output) {
  const artifacts = Object.values(manifest.artifacts ?? {});
  while (Buffer.byteLength(manifestText(manifest)) > MAX_MANIFEST_BYTES) {
    const oldest = artifacts
      .filter((artifact) => artifact.delta)
      .map((artifact) => ({ artifact, offer: artifact.delta.patches.at(-1) }))
      .sort((left, right) => compareVersions(left.offer.base_version, right.offer.base_version))[0];
    if (!oldest)
      throw new Error(`the native update manifest is larger than ${MAX_MANIFEST_BYTES} bytes`);
    warn(
      `Dropped the patch from ${oldest.offer.base_version} to keep the manifest within its limit.`,
    );
    oldest.artifact.delta.patches.pop();
    if (oldest.artifact.delta.patches.length === 0) delete oldest.artifact.delta;
    if (output) rmSync(join(output, basename(new URL(oldest.offer.url).pathname)), { force: true });
  }
  return manifest;
}

const USAGE = `usage:
  node scripts/native-update-patches.mjs generate --manifest MANIFEST.json --tool NATIVE_UPDATE_PATCH
    --bases RETAINED_DIR --archives NEW_ARCHIVES_DIR --output PATCH_DIR --patch-base-url HTTPS_URL
  node scripts/native-update-patches.mjs stale < retained-names`;

function options(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    if (!flag.startsWith("--") || argv[index + 1] === undefined || flag.slice(2) in values) {
      throw new Error(USAGE);
    }
    values[flag.slice(2)] = argv[index + 1];
  }
  return values;
}

function main() {
  const [command, ...rest] = process.argv.slice(2);
  if (command === "stale") {
    const names = readFileSync(0, "utf8").split(/\s+/).filter(Boolean);
    const stale = staleBases(names);
    if (stale.length) console.log(stale.join("\n"));
    return;
  }
  const args = options(rest);
  const required = ["manifest", "tool", "bases", "archives", "output", "patch-base-url"];
  if (command !== "generate" || required.some((name) => !args[name])) throw new Error(USAGE);
  if (new URL(args["patch-base-url"]).protocol !== "https:") {
    throw new Error("--patch-base-url must be an https URL");
  }
  const manifest = JSON.parse(readFileSync(args.manifest, "utf8"));
  const updated = generatePatches({
    manifest,
    tool: resolve(args.tool),
    bases: args.bases,
    archives: args.archives,
    output: args.output,
    patchBaseUrl: args["patch-base-url"],
  });
  const temporary = `${args.manifest}.patches-tmp`;
  writeFileSync(temporary, manifestText(updated));
  renameSync(temporary, args.manifest);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
