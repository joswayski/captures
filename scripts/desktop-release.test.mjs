import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  desktopLockChanged,
  desktopLockSnapshot,
  desktopReleaseImpact,
  isDesktopReleasePath,
  previousPreviewTag,
  releaseImpactBetween,
  releaseNotesBetween,
} from "./desktop-release.mjs";

function lock({ desktopVersion = "1.0.0", webVersion = "1.0.0", sharedVersion = "1.0.0" } = {}) {
  return {
    lockfileVersion: 3,
    packages: {
      "apps/desktop": {
        dependencies: { desktop: "*", shared: "*" },
        devDependencies: { builder: "*" },
      },
      "apps/web": { dependencies: { web: "*", shared: "*" } },
      "node_modules/desktop": { version: desktopVersion },
      "node_modules/web": { version: webVersion },
      "node_modules/shared": { version: sharedVersion },
      "node_modules/builder": { version: "1.0.0", dependencies: { shared: "*" } },
    },
  };
}

test("classifies installed-app paths without treating website, API, docs, or tests as releases", () => {
  for (const path of [
    "apps/desktop/ui/src/App.tsx",
    "apps/desktop/src-tauri/src/lib.rs",
    "crates/captures-video/src/lib.rs",
    "shared/design.css",
    ".cargo/config.toml",
    "Cargo.toml",
    "Cargo.lock",
  ]) {
    assert.equal(isDesktopReleasePath(path), true, path);
  }

  for (const path of [
    "apps/web/src/pages/Home.tsx",
    "apps/web/src/server/api.ts",
    "apps/desktop/ui/src/App.test.tsx",
    "README.md",
    "docs/images/preferences.jpg",
    ".github/workflows/release.yml",
    "scripts/release-version.mjs",
  ]) {
    assert.equal(isDesktopReleasePath(path), false, path);
  }
});

test("tracks only the package-lock dependency graph reachable from the desktop workspace", () => {
  assert.deepEqual(Object.keys(desktopLockSnapshot(lock()).packages), [
    "apps/desktop",
    "node_modules/builder",
    "node_modules/desktop",
    "node_modules/shared",
  ]);
  assert.equal(desktopLockChanged(lock(), lock({ webVersion: "2.0.0" })), false);
  assert.equal(desktopLockChanged(lock(), lock({ desktopVersion: "2.0.0" })), true);
  assert.equal(desktopLockChanged(lock(), lock({ sharedVersion: "2.0.0" })), true);
});

test("ignores API-only lock changes but releases desktop and shared changes", () => {
  assert.deepEqual(
    desktopReleaseImpact(
      ["apps/web/src/server/api.ts", "apps/web/package.json", "package-lock.json"],
      lock(),
      lock({ webVersion: "2.0.0" }),
    ),
    { shouldRelease: false, paths: [] },
  );
  assert.equal(desktopReleaseImpact(["shared/themes.ts"], null, null).shouldRelease, true);
  assert.equal(
    desktopReleaseImpact(
      ["apps/desktop/package.json", "package-lock.json"],
      lock(),
      lock({ desktopVersion: "2.0.0" }),
    ).shouldRelease,
    true,
  );
});

test("release notes omit skipped website commits from the next desktop update", () => {
  const directory = mkdtempSync(join(tmpdir(), "captures-desktop-release-notes-"));
  const originalDirectory = process.cwd();
  const runGit = (...args) => execFileSync("git", args, { cwd: directory, encoding: "utf8" }).trim();
  runGit("init");
  runGit("config", "user.name", "Captures Test");
  runGit("config", "user.email", "captures@example.com");
  writeFileSync(join(directory, "README.md"), "Captures\n");
  runGit("add", "README.md");
  runGit("commit", "-m", "Initial repository");
  const base = runGit("rev-parse", "HEAD");

  mkdirSync(join(directory, "apps/web/src"), { recursive: true });
  writeFileSync(join(directory, "apps/web/src/api.ts"), "export const api = true;\n");
  runGit("add", "apps/web/src/api.ts");
  runGit("commit", "-m", "Update the hosted API (#10)");
  const webHead = runGit("rev-parse", "HEAD");

  mkdirSync(join(directory, "apps/desktop/ui/src"), { recursive: true });
  writeFileSync(join(directory, "apps/desktop/ui/src/App.tsx"), "export const App = true;\n");
  runGit("add", "apps/desktop/ui/src/App.tsx");
  runGit("commit", "-m", "Fix desktop capture (#11)");
  const head = runGit("rev-parse", "HEAD");

  try {
    process.chdir(directory);
    const fallback = releaseNotesBetween(base, webHead, "joswayski/captures");
    assert.match(fallback, new RegExp(`Rebuilt desktop installers from commit \\[${webHead.slice(0, 7)}\\]`, "u"));

    const notes = releaseNotesBetween(base, head, "joswayski/captures");
    assert.doesNotMatch(notes, /hosted API/u);
    assert.match(notes, /Fix desktop capture \(\[#11\]\(https:\/\/github\.com\/joswayski\/captures\/pull\/11\)\)/u);

    rmSync(join(directory, "apps/desktop/ui/src/App.tsx"));
    runGit("add", "apps/desktop/ui/src/App.tsx");
    runGit("commit", "-m", "Remove obsolete desktop code (#12)");
    const deletedHead = runGit("rev-parse", "HEAD");
    assert.deepEqual(releaseImpactBetween(head, deletedHead), {
      shouldRelease: true,
      paths: ["apps/desktop/ui/src/App.tsx"],
    });
  } finally {
    process.chdir(originalDirectory);
  }
});

test("the Preview workflow gates builds and generates scoped notes", () => {
  const workflow = readFileSync(
    fileURLToPath(new URL("../.github/workflows/release.yml", import.meta.url)),
    "utf8",
  );
  assert.match(workflow, /should_release: \$\{\{ steps\.scope\.outputs\.should_release \}\}/u);
  assert.match(workflow, /if: needs\.queue\.outputs\.should_release == 'true'/u);
  assert.match(workflow, /desktop-release\.mjs changed/u);
  assert.match(workflow, /desktop-release\.mjs"? notes/u);
  assert.match(
    workflow,
    /name: Load the current desktop release helper[\s\S]*?ref: \$\{\{ github\.sha \}\}[\s\S]*?sparse-checkout: scripts\/desktop-release\.mjs/u,
  );
  assert.doesNotMatch(workflow, /generate_release_notes=true/u);
  assert.match(workflow, /concurrency:\n  group: captures-preview-main\n  cancel-in-progress: false\n\n/u);
  assert.doesNotMatch(workflow, /wait-preview-queue|github\.event\.before/u);
  assert.match(workflow, /release_sha="\$\{REQUESTED_SHA:-origin\/main\}"/u);
  assert.match(workflow, /BEFORE_SHA: \$\{\{ steps\.baseline\.outputs\.previous_tag \}\}/u);
  assert.match(workflow, /"\$generated_notes" \\\n\s+"\$PREVIOUS_TAG"/u);
});

test("batches unpublished desktop changes, fixes, and notes across docs-only pushes", () => {
  const directory = mkdtempSync(join(tmpdir(), "captures-preview-batch-"));
  const originalDirectory = process.cwd();
  const runGit = (...args) => execFileSync("git", args, { cwd: directory, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  const commit = (path, text) => {
    writeFileSync(join(directory, path), text);
    runGit("add", path);
    runGit("commit", "-m", text);
    return runGit("rev-parse", "HEAD");
  };
  const published = (tag_name) => ({ tag_name, draft: false, prerelease: true });
  try {
    runGit("init");
    runGit("config", "user.name", "Captures Test");
    runGit("config", "user.email", "captures@example.com");
    mkdirSync(join(directory, "apps/desktop"), { recursive: true });
    const first = commit("apps/desktop/app.txt", "Change 1 (#1)");
    runGit("tag", "v2026.09.05.1");
    const releases = [published("v2026.09.05.1")];
    commit("apps/desktop/app.txt", "Change 2 (#2)");
    runGit("tag", "v2026.09.05.2");
    releases.push({ ...published("v2026.09.05.2"), draft: true });
    commit("apps/desktop/app.txt", "Fix 3 (#3)");
    runGit("tag", "v2026.09.05.3"); // Orphan tag from an interrupted stage.
    const batch = commit("README.md", "Docs 4 (#4)");
    process.chdir(directory);
    assert.equal(previousPreviewTag(batch, releases), "v2026.09.05.1");
    assert.equal(releaseImpactBetween(first, batch).shouldRelease, true);
    assert.equal(releaseImpactBetween("", batch).shouldRelease, true, "bootstrap includes older desktop files");
    const notes = releaseNotesBetween(previousPreviewTag(batch, releases), batch, "joswayski/captures");
    assert.match(notes, /Change 2/u);
    assert.match(notes, /Fix 3/u);
    assert.doesNotMatch(notes, /Change 1|Docs 4/u);

    // A new push cannot alter the pinned batch. After publication it belongs
    // exclusively to the next batch, and repeated/docs-only runs are skipped.
    const next = commit("apps/desktop/app.txt", "Change 5 (#5)");
    runGit("tag", "v2026.09.05.4", batch);
    releases.push(published("v2026.09.05.4"));
    assert.equal(previousPreviewTag(batch, releases), "v2026.09.05.4");
    assert.equal(releaseImpactBetween("v2026.09.05.4", batch).shouldRelease, false);
    const nextNotes = releaseNotesBetween(previousPreviewTag(next, releases), next, "joswayski/captures");
    assert.match(nextNotes, /Change 5/u);
    assert.doesNotMatch(nextNotes, /Change 2|Fix 3/u);
    // Historical builds must not use a later published version as their base.
    assert.equal(previousPreviewTag(first, releases), "v2026.09.05.1");
    assert.equal(previousPreviewTag(next, []), "");
    runGit("tag", "v2026.09.05.5", next);
    releases.push(published("v2026.09.05.5"));
    const docs = commit("README.md", "Docs only (#6)");
    assert.equal(releaseImpactBetween(previousPreviewTag(docs, releases), docs).shouldRelease, false);
  } finally {
    process.chdir(originalDirectory);
    rmSync(directory, { recursive: true, force: true });
  }
});
