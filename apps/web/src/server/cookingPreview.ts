import {
  COOKING_STATUS_CACHE_MS,
  cookingPreviewShas,
  type CookingChange,
  type GitHubWorkflowRun,
} from "../cookingPreview.ts";

const REPO_API = "https://api.github.com/repos/joswayski/captures";
const PREVIEW_TAG = /^v(\d{4})\.(\d{2})\.(\d{2})\.([1-9]\d?)$/u;
const GITHUB_TIMEOUT_MS = 5_000;
/** How long to serve the fallback before retrying GitHub after a failed lookup. */
const FAILURE_RETRY_MS = 60_000;

type GitHubRelease = {
  draft: boolean;
  prerelease: boolean;
  tag_name: string;
  target_commitish: string;
};

type GitHubWorkflowRuns = {
  workflow_runs?: GitHubWorkflowRun[];
};

type CookingPreviewCache = {
  key: string;
  expiresAt: number;
  value: readonly string[];
};

let cookingPreviewCache: CookingPreviewCache | null = null;
let cookingPreviewInflight: { key: string; promise: Promise<string[]> } | null = null;

function previewVersion(tag: string) {
  const match = PREVIEW_TAG.exec(tag);
  if (!match) return null;
  return match.slice(1).map(Number);
}

function comparePreviewVersions(left: number[], right: number[]) {
  for (let index = 0; index < left.length; index += 1) {
    const order = left[index] - right[index];
    if (order !== 0) return order;
  }
  return 0;
}

async function githubJson<T>(path: string): Promise<T> {
  const response = await fetch(`${REPO_API}${path}`, {
    headers: {
      Accept: "application/vnd.github+json",
      "User-Agent": "captures-web",
      "X-GitHub-Api-Version": "2022-11-28",
    },
    // Homepage SSR awaits these lookups; a hung GitHub connection must not
    // stall page responses indefinitely.
    signal: AbortSignal.timeout(GITHUB_TIMEOUT_MS),
  });
  if (!response.ok) {
    throw new Error(`GitHub request failed (${response.status})`);
  }
  return (await response.json()) as T;
}

async function latestPublishedPreviewCommit(): Promise<string | null> {
  const releases = await githubJson<GitHubRelease[]>("/releases?per_page=40");
  let best: { version: number[]; tag: string; commitish: string } | null = null;

  for (const release of releases) {
    if (release.draft || !release.prerelease) continue;
    const version = previewVersion(release.tag_name);
    if (!version) continue;
    if (!best || comparePreviewVersions(version, best.version) > 0) {
      best = {
        version,
        tag: release.tag_name,
        commitish: release.target_commitish,
      };
    }
  }

  if (!best) return null;
  // Releases usually pin the full SHA; fall back if CI used a branch name.
  if (/^[0-9a-f]{40}$/iu.test(best.commitish)) return best.commitish.toLowerCase();

  try {
    const ref = await githubJson<{ object: { sha: string; type: string } }>(
      `/git/ref/tags/${encodeURIComponent(best.tag)}`,
    );
    if (ref.object.type === "commit") return ref.object.sha.toLowerCase();
    const tagObject = await githubJson<{ object: { sha: string } }>(`/git/tags/${ref.object.sha}`);
    return tagObject.object.sha.toLowerCase();
  } catch {
    return null;
  }
}

async function recentReleaseRuns(): Promise<GitHubWorkflowRun[]> {
  try {
    const payload = await githubJson<GitHubWorkflowRuns>(
      "/actions/workflows/release.yml/runs?per_page=30",
    );
    return payload.workflow_runs ?? [];
  } catch {
    // Actions API can be picky unauthenticated; releases alone still cover lagging publishes.
    return [];
  }
}

async function loadCookingPreviewShas(changes: readonly CookingChange[]): Promise<string[]> {
  const now = Date.now();
  const [publishedCommit, runs] = await Promise.all([
    latestPublishedPreviewCommit(),
    recentReleaseRuns(),
  ]);
  return cookingPreviewShas(changes, { publishedCommit, runs, now });
}

/** Resolve cooking SHAs, reusing the last GitHub lookup for an hour. */
export async function resolveCookingPreviewShas(
  changes: readonly CookingChange[],
): Promise<string[]> {
  if (changes.length === 0) return [];

  const cacheKey = changes.map((change) => change.sha).join(",");
  const now = Date.now();
  if (
    cookingPreviewCache &&
    cookingPreviewCache.key === cacheKey &&
    now < cookingPreviewCache.expiresAt
  ) {
    return [...cookingPreviewCache.value];
  }

  if (cookingPreviewInflight && cookingPreviewInflight.key === cacheKey) {
    return cookingPreviewInflight.promise;
  }

  const promise = loadCookingPreviewShas(changes)
    .then((value) => {
      cookingPreviewCache = {
        key: cacheKey,
        expiresAt: Date.now() + COOKING_STATUS_CACHE_MS,
        value,
      };
      return value;
    })
    .catch((error: unknown) => {
      // Back off instead of retrying GitHub on every request during an
      // outage. Serve the last known value (possibly expired) meanwhile.
      const fallback =
        cookingPreviewCache?.key === cacheKey ? cookingPreviewCache.value : [];
      cookingPreviewCache = {
        key: cacheKey,
        expiresAt: Date.now() + FAILURE_RETRY_MS,
        value: fallback,
      };
      throw error;
    })
    .finally(() => {
      if (cookingPreviewInflight?.promise === promise) {
        cookingPreviewInflight = null;
      }
    });

  cookingPreviewInflight = { key: cacheKey, promise };
  return promise;
}
