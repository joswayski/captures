// Per-file patch updates for the public and staging feeds, the way Caper
// does them (format and client: crates/captures-app/src/updater/patch.rs and
// crates/captures-app/src/updater/preview/delta.rs).
//
// `generate TARGET VERSION DIRECTORY [--staging]` runs after `collect` in a
// package job, with the updater signing key and GH_TOKEN. It unpacks the new
// updater artifact in DIRECTORY to the package folder users end up with
// (`Captures.app`, the installed Windows folder, the AppImage's AppDir), does
// the same for up to BASES earlier native builds of the same feed (the newest,
// then the last of each earlier day; see baseReleases),
// and writes `Captures-<version>-<target>.from-<base>.capturespatch` with its
// own signature for every base: unchanged files are copied from the installed
// package, changed ones are bsdiff or zstd deltas of the installed file, new
// ones are carried whole. Patches not smaller than half the full artifact are
// dropped. A failure is a warning, never a failed release: clients then
// download the full artifact. `manifest` in native-preview-build.mjs lists
// what it wrote.
import { execFileSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  unlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { TARGETS, patchName, updaterArtifactName } from "./native-preview-build.mjs";

const ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));
// Clients consider at most 16 offers (patch::MAX_OFFERS), and a patch applies
// only to the exact version it starts from. A busy day ships dozens of
// releases, so the newest RECENT builds cover apps that update often and the
// last build of each earlier day covers apps that update about once a day.
export const BASES = 15;
export const RECENT = 8;
const NATIVE_MARKER = "captures-native.json";
const PUBLIC_TAG = /^v\d{4}\.\d{2}\.\d{2}\.[1-9]\d?$/u;
const STAGING_TAG = /^native-staging-\d+\.\d+\.\d+$/u;
const PATCH = /\.capturespatch(\.sig)?$/u;

function compareVersions(left, right) {
  const a = left.split(".").map(Number);
  const b = right.split(".").map(Number);
  for (let index = 0; index < 3; index += 1) if (a[index] !== b[index]) return a[index] - b[index];
  return 0;
}

// CalVer `YYYY.MM.DDNN`: the day a build was released.
const day = (version) => {
  const [year, month, build] = version.split(".");
  return `${year}.${month}.${Math.floor(Number(build) / 100)}`;
};

/**
 * Earlier native builds of one feed that installed apps may run, newest first:
 * staging prereleases, or public releases carrying the native runtime marker
 * (Tauri installs cannot apply patches). Up to BASES of them: the RECENT
 * newest, then the last build of each earlier day.
 */
export function baseReleases(releases, target, version, staging) {
  const suffix = TARGETS[target].suffix;
  const pattern = new RegExp(
    `^Captures-(\\d+\\.\\d+\\.\\d+)-${target.replaceAll(".", "\\.")}${suffix.replaceAll(".", "\\.")}$`,
    "u",
  );
  const uploaded = (release, name) =>
    release.assets?.some((asset) => asset.name === name && asset.state === "uploaded");
  const candidates = releases
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
    .filter((base, index, all) => index === 0 || all[index - 1].version !== base.version);
  const chosen = new Set(candidates.slice(0, RECENT));
  const days = new Set([...chosen].map((base) => day(base.version)));
  for (const base of candidates)
    if (chosen.size < BASES && !days.has(day(base.version))) {
      chosen.add(base);
      days.add(day(base.version));
    }
  // Without enough earlier days, the next newest builds fill the rest.
  for (const base of candidates) if (chosen.size < BASES) chosen.add(base);
  return candidates.filter((base) => chosen.has(base));
}

/** Entries 7-Zip shows in an NSIS installer that never reach the install
 * folder: plugin files, the decompiled script, and the uninstaller NSIS
 * writes at install time (its bytes differ per install). */
export function installerOnly(name) {
  return name.startsWith("$") || name.startsWith("[") || name.toLowerCase() === "uninstall.exe";
}

function run(command, args, options = {}) {
  return execFileSync(command, args, {
    stdio: ["ignore", "pipe", "inherit"],
    maxBuffer: 256 * 1024 * 1024,
    ...options,
  });
}

/** The package folder an installed copy of `artifact` holds, unpacked into the
 * new empty folder `into`. */
export function unpack(target, artifact, into) {
  mkdirSync(into, { recursive: true });
  if (target.includes("apple-darwin")) {
    run("tar", ["-xzf", artifact, "-C", into]);
    const [app, ...others] = readdirSync(into);
    if (!app?.endsWith(".app") || others.length)
      throw new Error(`${artifact} is not one app bundle`);
    return join(into, app);
  }
  if (target.includes("windows")) {
    const folder = join(into, "Captures");
    run("7z", ["x", "-y", `-o${folder}`, artifact]);
    for (const name of readdirSync(folder))
      if (installerOnly(name)) rmSync(join(folder, name), { recursive: true, force: true });
    return folder;
  }
  // The AppImage runtime extracts the squashfs a running AppImage mounts.
  const image = join(into, "Captures.AppImage");
  copyFileSync(artifact, image);
  chmodSync(image, 0o755);
  const env = { ...process.env };
  delete env.APPIMAGE_EXTRACT_AND_RUN;
  run(image, ["--appimage-extract"], { cwd: into, env, stdio: ["ignore", "ignore", "inherit"] });
  unlinkSync(image);
  const folder = join(into, "Captures.AppDir");
  renameSync(join(into, "squashfs-root"), folder);
  return folder;
}

export function generate(target, version, directory, staging) {
  const suffix = target.includes("windows") ? ".exe" : "";
  const helper = join(ROOT, "target", target, "release", `native_preview_update_helper${suffix}`);
  const artifact = join(directory, updaterArtifactName(target, version));
  const fullSize = statSync(artifact).size;
  const work = mkdtempSync(join(process.env.RUNNER_TEMP ?? tmpdir(), "captures-patches-"));
  try {
    const next = unpack(target, artifact, join(work, "release"));
    const releases = JSON.parse(
      run("gh", [
        "api",
        "--paginate",
        "--slurp",
        `repos/${process.env.GITHUB_REPOSITORY}/releases?per_page=100`,
      ]),
    ).flat();
    const bases = baseReleases(releases, target, version, staging);
    if (!bases.length) console.log(`No earlier ${target} builds to patch from.`);
    for (const [index, base] of bases.entries()) {
      const output = join(directory, patchName(target, version, base.version));
      const scratch = join(work, `base-${index}`);
      try {
        const downloads = join(scratch, "download");
        run("gh", ["release", "download", base.tag, "--pattern", base.name, "--dir", downloads]);
        const from = unpack(target, join(downloads, base.name), join(scratch, "package"));
        const summary = JSON.parse(
          run(helper, [
            "--make-patch",
            target,
            base.version,
            from,
            version,
            next,
            output,
          ]).toString(),
        );
        if (summary.size * 2 >= fullSize) {
          unlinkSync(output);
          console.log(`Dropped the ${base.version} patch: ${summary.size} of ${fullSize} bytes.`);
          continue;
        }
        run(process.execPath, [
          join(ROOT, "node_modules/@tauri-apps/cli/tauri.js"),
          "signer",
          "sign",
          output,
        ]);
        run(helper, ["--verify", version, output, `${output}.sig`]);
        console.log(
          `Patch from ${base.version}: ${summary.size} of ${fullSize} bytes ` +
            `(${summary.copied} copied, ${summary.bsdiff} bsdiff, ${summary.zstd} zstd, ${summary.whole} whole).`,
        );
      } catch (error) {
        rmSync(output, { force: true });
        rmSync(`${output}.sig`, { force: true });
        console.log(`::warning::No ${target} patch from ${base.version}: ${error.message}`);
      } finally {
        rmSync(scratch, { recursive: true, force: true });
      }
    }
  } catch (error) {
    for (const name of readdirSync(directory))
      if (PATCH.test(name)) rmSync(join(directory, name), { force: true });
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
