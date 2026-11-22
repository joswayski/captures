import assert from "node:assert/strict";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, posix } from "node:path";
import { test } from "vitest";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const helperPath = fileURLToPath(new URL("./github-release-assets.mjs", import.meta.url));
const workflowPath = fileURLToPath(new URL("../.github/workflows/release.yml", import.meta.url));
const RELATIVE_IMPORT = /from\s+["'](\.\/[^"']+)["']/gu;
const SPARSE_CHECKOUT =
  /name: Load the current release asset helper[\s\S]*?sparse-checkout:\s*\|\n((?:[ \t]+scripts\/.+\n)+)/u;

function macosHelperSparseCheckout() {
  const workflow = readFileSync(workflowPath, "utf8");
  const match = workflow.match(SPARSE_CHECKOUT);
  assert.ok(match, "macOS package job must sparse-checkout the current release asset helper");
  return match[1]
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}

function localImports(sourcePath) {
  const source = readFileSync(sourcePath, "utf8");
  return [...source.matchAll(RELATIVE_IMPORT)].map((match) => match[1]);
}

test("macOS historical helper sparse checkout includes every local import", () => {
  const listed = macosHelperSparseCheckout();
  assert.ok(listed.includes("scripts/github-release-assets.mjs"));
  for (const specifier of localImports(helperPath)) {
    const relativePath = posix.join("scripts", specifier.replace(/^\.\//u, ""));
    assert.ok(
      listed.includes(relativePath),
      `${relativePath} is imported by github-release-assets.mjs but missing from the macOS helper sparse checkout`,
    );
  }
});

test("updater URLs use the workflow CalVer tag, not GitHub draft tag_name", () => {
  const workflow = readFileSync(workflowPath, "utf8");
  assert.match(
    workflow,
    /TAG: \$\{\{ needs\.prepare\.outputs\.tag \}\}[\s\S]*?native-preview-build\.mjs manifest release-assets "\$APP_VERSION"/u,
  );
  assert.match(
    workflow,
    /rewrite-updater-urls \\\n\s+"\$newest_id" \\\n\s+"\$channel_assets\/latest\.json" \\\n\s+"\$newest_tag"/u,
  );
});

test("publishing never resends the release commit, which GitHub refuses after a workflow change", () => {
  // GITHUB_TOKEN cannot hold the workflows permission. GitHub requires it for
  // a release update naming a commit whose workflow files differ from main's,
  // so a workflow change pushed mid-release turned the publish into a 403.
  const workflow = readFileSync(workflowPath, "utf8");
  const publish = workflow.slice(
    workflow.indexOf("\n  publish:"),
    workflow.indexOf("\n  channel:"),
  );
  assert.match(publish, /-F draft=false/u);
  assert.doesNotMatch(publish, /-[fF] target_commitish=/u);
  // The downloads page may record it, but only in a call that cannot fail the job.
  const channel = workflow.slice(
    workflow.indexOf("\n  channel:"),
    workflow.indexOf("\n  cleanup:"),
  );
  // Each `gh api` command with its backslash-continued lines.
  const commands = [...channel.matchAll(/gh api \\\n(?:.*\\\n)*.*\n/gu)].map((match) => match[0]);
  const recording = commands.filter(
    (command) => command.includes("--method PATCH") && command.includes("target_commitish"),
  );
  assert.equal(recording.length, 1);
  assert.doesNotMatch(recording[0], /draft=|-f body=|-f name=/u);
  assert.match(recording[0], /\|\| echo "::warning::/u);
});

test("the sparse helper checkout can load without the rest of the repository", async () => {
  const listed = macosHelperSparseCheckout();
  const directory = mkdtempSync(join(tmpdir(), "captures-release-tools-"));
  try {
    for (const relativePath of listed) {
      const destination = join(directory, relativePath);
      mkdirSync(dirname(destination), { recursive: true });
      cpSync(join(root, relativePath), destination);
    }
    const isolatedHelper = join(directory, "scripts", "github-release-assets.mjs");
    await import(pathToFileURL(isolatedHelper).href);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("release assets are read past the first page of 100", async () => {
  const { releaseAssets } = await import(pathToFileURL(helperPath).href);
  const all = Array.from({ length: 150 }, (_, index) => ({ id: index, name: `asset-${index}` }));
  const requested = [];
  const saved = {
    fetch: globalThis.fetch,
    repository: process.env.GITHUB_REPOSITORY,
    token: process.env.GH_TOKEN,
  };
  process.env.GITHUB_REPOSITORY = "owner/name";
  process.env.GH_TOKEN = "token";
  globalThis.fetch = async (url) => {
    requested.push(String(url));
    const page = Number(new URL(url).searchParams.get("page") ?? "1");
    return new Response(JSON.stringify(all.slice((page - 1) * 100, page * 100)), { status: 200 });
  };
  try {
    const assets = await releaseAssets(7);
    assert.deepEqual(
      assets.map((asset) => asset.name),
      all.map((asset) => asset.name),
    );
    assert.equal(requested.length, 2);
  } finally {
    globalThis.fetch = saved.fetch;
    for (const [key, value] of [
      ["GITHUB_REPOSITORY", saved.repository],
      ["GH_TOKEN", saved.token],
    ]) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
});
