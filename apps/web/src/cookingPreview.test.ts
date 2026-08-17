import assert from "node:assert/strict";
import test from "node:test";

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

function run(
  sha: string,
  status: string,
  conclusion: string | null = null,
): GitHubWorkflowRun {
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
  assert.deepEqual([...buckets.failed].sort(), ["eee", "fff", "ggg"]);
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
  const shas = cookingPreviewShas(
    [change("published", HOUR), change("older", 2 * HOUR)],
    { publishedCommit: "PUBLISHED", runs: [], now: NOW },
  );

  assert.deepEqual(shas, []);
});

test("hides finished failures even when they are newer than the published Preview", () => {
  const shas = cookingPreviewShas(
    [change("failed", 20 * 60 * 1_000), change("published", HOUR)],
    {
      publishedCommit: "published",
      runs: [run("failed", "completed", "failure")],
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
    [change("one", HOUR), change("building", 30 * 60 * 1_000), change("two", COOKING_MAX_AGE_MS + 1)],
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
