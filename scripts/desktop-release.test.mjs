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
});
