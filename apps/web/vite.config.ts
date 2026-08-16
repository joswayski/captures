import { tanstackStart } from "@tanstack/react-start/plugin/vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { nitro } from "nitro/vite";
import { defineConfig } from "vite";

const REPOSITORY = "joswayski/captures";
const CHANGE_COUNT = 10;
/** Fetch extra commits so Dependabot merges can be dropped without under-filling the list. */
const FETCH_COUNT = 30;

type GitHubCommit = {
  sha: string;
  html_url: string;
  author: { login: string } | null;
  commit: {
    message: string;
    committer: { date: string } | null;
    author: { name?: string; date: string } | null;
  };
};

type LatestChange = {
  sha: string;
  title: string;
  url: string;
  committedAt: string;
  pullRequest: number | null;
};

function pullRequestNumber(title: string) {
  return (
    title.match(/\(#(\d+)\)$/u)?.[1] ??
    title.match(/^Merge pull request #(\d+)/u)?.[1] ??
    null
  );
}

function isDependabotCommit(entry: GitHubCommit): boolean {
  const login = entry.author?.login?.toLowerCase() ?? "";
  if (login === "dependabot[bot]" || login.startsWith("dependabot")) {
    return true;
  }

  const authorName = entry.commit.author?.name?.toLowerCase() ?? "";
  return authorName === "dependabot[bot]" || authorName.startsWith("dependabot");
}

function toLatestChange(entry: GitHubCommit): LatestChange {
  const title = entry.commit.message.split("\n", 1)[0]?.trim();
  const committedAt = entry.commit.committer?.date ?? entry.commit.author?.date;

  if (!entry.sha || !entry.html_url || !title || !committedAt) {
    throw new Error("GitHub returned an incomplete commit entry");
  }

  const prNumber = pullRequestNumber(title);

  return {
    sha: entry.sha,
    title: prNumber ? title.replace(/\s+\(#\d+\)$/u, "") : title,
    url: prNumber
      ? `https://github.com/${REPOSITORY}/pull/${prNumber}`
      : entry.html_url,
    committedAt,
    pullRequest: prNumber ? Number(prNumber) : null,
  };
}

async function fetchLatestChanges(): Promise<LatestChange[]> {
  const url = new URL(`https://api.github.com/repos/${REPOSITORY}/commits`);
  url.searchParams.set("sha", "main");
  url.searchParams.set("per_page", String(FETCH_COUNT));

  const response = await fetch(url, {
    headers: {
      Accept: "application/vnd.github+json",
      "User-Agent": "captures-web-build",
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });

  if (!response.ok) {
    throw new Error(`GitHub history request failed with ${response.status}`);
  }

  const entries = (await response.json()) as GitHubCommit[];
  if (!Array.isArray(entries) || entries.length === 0) {
    throw new Error("GitHub returned no commits for main");
  }

  const productChanges = entries
    .filter((entry) => !isDependabotCommit(entry))
    .map(toLatestChange)
    .slice(0, CHANGE_COUNT);

  if (productChanges.length === 0) {
    throw new Error("GitHub returned no non-Dependabot commits for main");
  }

  return productChanges;
}

export default defineConfig(async () => {
  const latestChanges = await fetchLatestChanges();
  console.log(`Fetched ${latestChanges.length} latest changes from the GitHub API.`);

  return {
    plugins: [
      tanstackStart({
        prerender: {
          enabled: false,
        },
        server: {
          build: {
            inlineCss: true,
          },
        },
      }),
      react(),
      tailwindcss(),
      nitro({
        serverDir: "server",
        routeRules: {
          "/assets/**": {
            headers: {
              "cache-control": "public, max-age=31536000, immutable",
            },
          },
          "/favicon.png": {
            headers: {
              "cache-control": "public, max-age=86400",
            },
          },
          "/icon.svg": {
            headers: {
              "cache-control": "public, max-age=86400",
            },
          },
        },
      }),
    ],
    define: {
      __LATEST_CHANGES__: JSON.stringify(latestChanges),
    },
    server: {
      port: 5174,
    },
  };
});
