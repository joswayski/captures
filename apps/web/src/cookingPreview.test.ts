import assert from "node:assert/strict";
import { test, vi } from "vitest";

import {
  COOKING_MAX_AGE_MS,
  cookingPreviewShas,
  isWithinCookingWindow,
  releaseRunBuckets,
  type CookingChange,
  type GitHubWorkflowRun,
} from "./cookingPreview.ts";

const HOUR = 60 * 60 * 1_000;
const NOW = Date.parse("2026-08-17T12:00:00.000Z");

function change(sha: string, ageMs: number): CookingChange {
  return {
    sha,
    committedAt: new Date(NOW - ageMs).toISOString(),
  };
}

function run(sha: string, status: string, conclusion: string | null = null): GitHubWorkflowRun {
  return { head_sha: sha, status, conclusion };
}

test("isWithinCookingWindow rejects invalid dates and old merges", () => {
  assert.equal(isWithinCookingWindow("2026-08-17T11:00:00.000Z", NOW), true);
  assert.equal(isWithinCookingWindow(new Date(NOW - COOKING_MAX_AGE_MS).toISOString(), NOW), true);
  assert.equal(
    isWithinCookingWindow(new Date(NOW - COOKING_MAX_AGE_MS - 1).toISOString(), NOW),
    false,
  );
  assert.equal(isWithinCookingWindow("not-a-date", NOW), false);
});

test("releaseRunBuckets treats queued runs as building and retries as success", () => {
  const buckets = releaseRunBuckets([
    run("aaa", "queued"),
    run("bbb", "in_progress"),
    run("ccc", "pending"),
    run("ddd", "completed", "failure"),
    run("ddd", "completed", "success"),
    run("eee", "completed", "cancelled"),
    run("fff", "completed", "timed_out"),
    run("ggg", "completed", "startup_failure"),
    run("HHH", "completed", "success"),
  ]);

  assert.deepEqual([...buckets.building].sort(), ["aaa", "bbb", "ccc"]);
  assert.deepEqual([...buckets.failed].sort(), ["fff", "ggg"]);
  assert.deepEqual([...buckets.succeeded].sort(), ["ddd", "hhh"]);
});

test("marks commits newer than the published Preview as cooking", () => {
  const shas = cookingPreviewShas(
    [change("new", 30 * 60 * 1_000), change("published", 2 * HOUR), change("older", 3 * HOUR)],
    { publishedCommit: "published", runs: [], now: NOW },
  );

  assert.deepEqual(shas, ["new"]);
});

test("does not cook the published commit or older history", () => {
  const shas = cookingPreviewShas([change("published", HOUR), change("older", 2 * HOUR)], {
    publishedCommit: "PUBLISHED",
    runs: [],
    now: NOW,
  });

  assert.deepEqual(shas, []);
});

test("hides finished failures even when they are newer than the published Preview", () => {
  const shas = cookingPreviewShas([change("failed", 20 * 60 * 1_000), change("published", HOUR)], {
    publishedCommit: "published",
    runs: [run("failed", "completed", "failure")],
    now: NOW,
  });

  assert.deepEqual(shas, []);
});

test("hides successful scope-only runs that intentionally publish no Preview", () => {
  const shas = cookingPreviewShas(
    [change("website-only", 20 * 60 * 1_000), change("published", HOUR)],
    {
      publishedCommit: "published",
      runs: [run("website-only", "completed", "success")],
      now: NOW,
    },
  );

  assert.deepEqual(shas, []);
});

test("badges an in-progress run inside the cooking window", () => {
  const shas = cookingPreviewShas([change("building", HOUR), change("published", 2 * HOUR)], {
    publishedCommit: "published",
    runs: [run("building", "in_progress")],
    now: NOW,
  });

  assert.deepEqual(shas, ["building"]);
});

test("does not badge a building or unpublished merge older than the cooking window", () => {
  const shas = cookingPreviewShas(
    [change("stale-new", COOKING_MAX_AGE_MS + HOUR), change("stale-build", COOKING_MAX_AGE_MS + 1)],
    {
      publishedCommit: "published",
      runs: [run("stale-build", "queued")],
      now: NOW,
    },
  );

  assert.deepEqual(shas, []);
});

test("without a published Preview, only recent in-progress runs cook", () => {
  const shas = cookingPreviewShas(
    [
      change("one", HOUR),
      change("building", 30 * 60 * 1_000),
      change("two", COOKING_MAX_AGE_MS + 1),
    ],
    {
      publishedCommit: null,
      runs: [run("building", "in_progress")],
      now: NOW,
    },
  );

  assert.deepEqual(shas, ["building"]);
});

test("returns no cooking SHAs for an empty change list", () => {
  assert.deepEqual(
    cookingPreviewShas([], { publishedCommit: "published", runs: [], now: NOW }),
    [],
  );
});

test("superseded runs and failed earlier attempts cook as part of the next batch", () => {
  const changes = [
    change("five", HOUR),
    change("four", HOUR),
    change("three", HOUR),
    change("published", 2 * HOUR),
  ];
  const runs = [
    run("five", "pending"),
    run("four", "completed", "cancelled"),
    run("three", "completed", "failure"),
  ];
  assert.deepEqual(cookingPreviewShas(changes, { publishedCommit: "published", runs, now: NOW }), [
    "five",
    "four",
    "three",
  ]);
  // The published snapshot, not any stale workflow event SHA, ends cooking.
  assert.deepEqual(cookingPreviewShas(changes, { publishedCommit: "five", runs, now: NOW }), []);
});

test("publishing status uses the latest Tauri release, ignoring a newer native archive", async () => {
  const shas = ["3".repeat(40), "2".repeat(40), "1".repeat(40)];
  const releases = [
    { tag_name: "v2026.10.19.1", target_commitish: shas[0] },
    {
      tag_name: "v2026.10.18.2",
      target_commitish: shas[1],
      assets: [{ name: "captures-tauri.json", state: "uploaded" }],
    },
    { tag_name: "v2026.10.11.4", target_commitish: shas[2] },
  ].map((release) => ({ ...release, draft: false, prerelease: true }));
  vi.stubGlobal("fetch", async (url: string) =>
    Response.json(url.includes("/releases?") ? releases : { workflow_runs: [] }),
  );
  try {
    const { resolveCookingPreviewShas } = await import("./server/cookingPreview.ts");
    const changes = shas.map((sha) => ({ sha, committedAt: new Date().toISOString() }));
    assert.deepEqual(await resolveCookingPreviewShas(changes), [shas[0]]);
  } finally {
    vi.unstubAllGlobals();
  }
});
