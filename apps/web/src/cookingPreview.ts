/** Hide "still cooking" once a merge is this old — avoids stuck/failed publishes lingering. */
export const COOKING_MAX_AGE_MS = 4 * 60 * 60 * 1_000;
/** How long the server reuses a Preview publishing lookup. */
export const COOKING_STATUS_CACHE_MS = 60 * 60 * 1_000;

export type CookingChange = {
  sha: string;
  committedAt: string;
};

export type GitHubWorkflowRun = {
  head_sha: string;
  status: string;
  conclusion: string | null;
};

export function isWithinCookingWindow(committedAt: string, now: number) {
  const committedMs = new Date(committedAt).getTime();
  if (Number.isNaN(committedMs)) return false;
  return now - committedMs <= COOKING_MAX_AGE_MS;
}

export function releaseRunBuckets(runs: readonly GitHubWorkflowRun[]) {
  const building = new Set<string>();
  const failed = new Set<string>();
  const succeeded = new Set<string>();

  for (const run of runs) {
    const sha = run.head_sha?.toLowerCase();
    if (!sha) continue;

    if (run.status === "in_progress" || run.status === "queued" || run.status === "pending") {
      building.add(sha);
      continue;
    }

    if (run.status !== "completed") continue;

    if (run.conclusion === "success") {
      succeeded.add(sha);
    } else if (
      run.conclusion === "failure" ||
      run.conclusion === "timed_out" ||
      run.conclusion === "startup_failure"
    ) {
      // Cancelled pending runs are superseded by the next release batch, not
      // failed changes. Only mark failures if this SHA never also succeeded.
      if (!succeeded.has(sha)) failed.add(sha);
    }
  }

  // A later success for the same SHA clears failure.
  for (const sha of succeeded) failed.delete(sha);

  return { building, failed, succeeded };
}

/** Commits still waiting on a finished Preview publish (newer than the latest release, or actively building). */
export function cookingPreviewShas(
  changes: readonly CookingChange[],
  input: {
    publishedCommit: string | null;
    runs: readonly GitHubWorkflowRun[];
    now: number;
  },
): string[] {
  if (changes.length === 0) return [];

  const publishedCommit = input.publishedCommit?.toLowerCase() ?? null;
  const { building, failed, succeeded } = releaseRunBuckets(input.runs);
  const cooking = new Set<string>();
  let seenPublished = publishedCommit === null;
  const batchHeadIndex = changes.findIndex((change) => building.has(change.sha.toLowerCase()));

  for (const [index, change] of changes.entries()) {
    const sha = change.sha.toLowerCase();
    const recentEnough = isWithinCookingWindow(change.committedAt, input.now);

    // Publication wins over old workflow metadata, including superseded runs
    // whose event SHA differs from the main snapshot that actually shipped.
    if (publishedCommit && sha === publishedCommit) {
      seenPublished = true;
    }
    const includedInBatch = !seenPublished && batchHeadIndex !== -1 && index >= batchHeadIndex;

    if (recentEnough && (includedInBatch || (!publishedCommit && building.has(sha)))) {
      cooking.add(change.sha);
    }

    if (!seenPublished) {
      if (recentEnough && !failed.has(sha) && !succeeded.has(sha)) {
        // Newer than the latest published Preview and still unresolved. A
        // successful scope-only run intentionally has no release to wait for.
        cooking.add(change.sha);
      }
    }
  }

  return [...cooking];
}
