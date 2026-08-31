import { appendFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const ZERO_SHA = /^0{40}$/u;
const DESKTOP_ROOTS = [".cargo/", "apps/desktop/", "crates/", "shared/"];
const DESKTOP_EXACT_PATHS = new Set(["Cargo.lock", "Cargo.toml"]);
const TEST_ONLY_PATH = /(?:^|\/)[^/]+\.test\.(?:[cm]?[jt]sx?)$/u;

/** Files whose contents can change the installed desktop app. */
export function isDesktopReleasePath(path) {
  const normalized = path.replace(/^\.\//u, "");
  if (TEST_ONLY_PATH.test(normalized)) return false;
  return DESKTOP_EXACT_PATHS.has(normalized)
    || DESKTOP_ROOTS.some((root) => normalized.startsWith(root));
}

function dependencyNames(pkg) {
  return Object.keys({
    ...pkg?.dependencies,
    ...pkg?.devDependencies,
    ...pkg?.optionalDependencies,
    ...pkg?.peerDependencies,
  });
}

function resolvePackage(packages, from, dependency) {
  let directory = from;
  while (true) {
    const candidate = directory
      ? `${directory}/node_modules/${dependency}`
      : `node_modules/${dependency}`;
    if (packages[candidate]) return candidate;
    if (!directory) return null;
    const slash = directory.lastIndexOf("/");
    directory = slash === -1 ? "" : directory.slice(0, slash);
  }
}

/** The package-lock subset that can affect the desktop bundle and its build. */
export function desktopLockSnapshot(lock) {
  const packages = lock?.packages;
  if (!packages || !packages["apps/desktop"]) {
    throw new Error("package-lock.json is missing the apps/desktop workspace");
  }

  const queue = ["apps/desktop"];
  const visited = new Set();
  while (queue.length > 0) {
    const location = queue.shift();
    if (visited.has(location)) continue;
    visited.add(location);
    for (const dependency of dependencyNames(packages[location])) {
      const resolved = resolvePackage(packages, location, dependency);
      if (resolved && !visited.has(resolved)) queue.push(resolved);
    }
  }

  return {
    lockfileVersion: lock.lockfileVersion,
    packages: Object.fromEntries(
      [...visited]
        .sort()
        .map((location) => [location, packages[location]]),
    ),
  };
}

export function desktopLockChanged(before, after) {
  if (!before || !after) return true;
  return JSON.stringify(desktopLockSnapshot(before)) !== JSON.stringify(desktopLockSnapshot(after));
}

export function desktopReleaseImpact(paths, beforeLock, afterLock) {
  const direct = paths.filter(isDesktopReleasePath);
  const lockChanged = paths.includes("package-lock.json")
    && desktopLockChanged(beforeLock, afterLock);
  return {
    shouldRelease: direct.length > 0 || lockChanged,
    paths: [...direct, ...(lockChanged ? ["package-lock.json (desktop dependency graph)"] : [])],
  };
}

function git(args, options = {}) {
  return execFileSync("git", args, { encoding: "utf8", ...options }).trim();
}

function refExists(ref) {
  if (!ref || ZERO_SHA.test(ref)) return false;
  try {
    execFileSync("git", ["rev-parse", "--verify", `${ref}^{commit}`], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function fileAt(ref, path) {
  if (!refExists(ref)) return null;
  try {
    return JSON.parse(git(["show", `${ref}:${path}`]));
  } catch {
    return null;
  }
}

function changedPaths(before, after) {
  const args = refExists(before)
    ? ["diff", "--name-only", "--diff-filter=ACMRTUXB", before, after]
    : ["diff-tree", "--root", "--no-commit-id", "--name-only", "-r", after];
  const output = git(args);
  return output ? output.split(/\r?\n/u) : [];
}

export function releaseImpactBetween(before, after) {
  const paths = changedPaths(before, after);
  return desktopReleaseImpact(
    paths,
    paths.includes("package-lock.json") ? fileAt(before, "package-lock.json") : null,
    paths.includes("package-lock.json") ? fileAt(after, "package-lock.json") : null,
  );
}

function commitMessage(commit) {
  const [subject, ...body] = git(["show", "-s", "--format=%s%n%b", commit]).split(/\r?\n/u);
  return { subject, body: body.join("\n").trim() };
}

function firstParent(commit) {
  try {
    return git(["rev-parse", `${commit}^1`]);
  } catch {
    return "";
  }
}

function noteForCommit(commit, repository) {
  const { subject, body } = commitMessage(commit);
  const squash = /^(.*?) \(#(\d+)\)$/u.exec(subject);
  if (squash) {
    const [, title, number] = squash;
    return { key: `pr-${number}`, text: `* ${title} ([#${number}](https://github.com/${repository}/pull/${number}))` };
  }

  const merge = /^Merge pull request #(\d+)\b/u.exec(subject);
  if (merge) {
    const number = merge[1];
    const title = body.split(/\r?\n/u).find((line) => line.trim())?.trim() ?? subject;
    return { key: `pr-${number}`, text: `* ${title} ([#${number}](https://github.com/${repository}/pull/${number}))` };
  }

  const short = commit.slice(0, 7);
  return { key: `commit-${commit}`, text: `* ${subject} ([${short}](https://github.com/${repository}/commit/${commit}))` };
}

export function releaseNotes(commits, repository) {
  const seen = new Set();
  const notes = [];
  for (const commit of commits) {
    const note = noteForCommit(commit, repository);
    if (seen.has(note.key)) continue;
    seen.add(note.key);
    notes.push(note.text);
  }
  if (notes.length === 0) throw new Error("desktop release range contains no installed-app changes");
  return `## What's Changed\n${notes.join("\n")}\n`;
}

export function previousPreviewTag(head) {
  const parent = firstParent(head);
  if (!parent) return "";
  try {
    return git([
      "describe",
      "--tags",
      "--abbrev=0",
      "--match",
      "v[0-9][0-9][0-9][0-9].[0-9][0-9].[0-9][0-9].[1-9]*",
      parent,
    ]);
  } catch {
    return "";
  }
}

export function releaseNotesBetween(before, after, repository) {
  const range = refExists(before) ? `${before}..${after}` : after;
  const output = git(["rev-list", "--reverse", range]);
  const qualifying = (output ? output.split(/\r?\n/u) : [])
    .filter((commit) => releaseImpactBetween(firstParent(commit), commit).shouldRelease);
  return releaseNotes(qualifying, repository);
}

function appendOutput(entries) {
  const output = process.env.GITHUB_OUTPUT;
  if (!output) return;
  appendFileSync(output, `${Object.entries(entries).map(([key, value]) => `${key}=${value}`).join("\n")}\n`);
}

function main() {
  const [command, ...args] = process.argv.slice(2);
  if (command === "changed") {
    const [before, after] = args;
    if (!after) throw new Error("usage: desktop-release.mjs changed <before> <after>");
    const impact = releaseImpactBetween(before, after);
    appendOutput({ should_release: String(impact.shouldRelease) });
    process.stdout.write(`${JSON.stringify(impact)}\n`);
    return;
  }

  if (command === "notes") {
    const [after, repository, outputPath] = args;
    if (!after || !repository || !outputPath) {
      throw new Error("usage: desktop-release.mjs notes <after> <repository> <output-path>");
    }
    const before = previousPreviewTag(after);
    writeFileSync(outputPath, releaseNotesBetween(before, after, repository));
    process.stdout.write(`Generated desktop release notes since ${before || "repository start"}.\n`);
    return;
  }

  throw new Error("usage: desktop-release.mjs <changed|notes> ...");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
