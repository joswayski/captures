import assert from "node:assert/strict";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, posix } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const helperPath = fileURLToPath(new URL("./github-release-assets.mjs", import.meta.url));
const workflowPath = fileURLToPath(new URL("../.github/workflows/release.yml", import.meta.url));
const RELATIVE_IMPORT = /from\s+["'](\.\/[^"']+)["']/gu;
const SPARSE_CHECKOUT = /name: Load the current release asset helper[\s\S]*?sparse-checkout:\s*\|\n((?:[ \t]+scripts\/.+\n)+)/u;

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
