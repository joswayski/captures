import { access, writeFile } from "node:fs/promises";

const REPOSITORY = "joswayski/captures";
const CHANGE_COUNT = 6;
const outputUrl = new URL("./src/latest-changes.ts", import.meta.url);

function pullRequestNumber(title) {
  return (
    title.match(/\(#(\d+)\)$/u)?.[1] ??
    title.match(/^Merge pull request #(\d+)/u)?.[1] ??
    null
  );
}

function toLatestChange(entry) {
  const title = entry.commit?.message?.split("\n", 1)[0]?.trim();
  const committedAt = entry.commit?.committer?.date ?? entry.commit?.author?.date;

  if (!entry.sha || !entry.html_url || !title || !committedAt) {
    throw new Error("GitHub returned an incomplete commit entry");
  }

  const prNumber = pullRequestNumber(title);
  const displayTitle = prNumber ? title.replace(/\s+\(#\d+\)$/u, "") : title;

  return {
    sha: entry.sha.slice(0, 7),
    title: displayTitle,
    url: prNumber
      ? `https://github.com/${REPOSITORY}/pull/${prNumber}`
      : entry.html_url,
    committedAt,
    pullRequest: prNumber ? Number(prNumber) : null,
  };
}

async function fetchLatestChanges() {
  const url = new URL(`https://api.github.com/repos/${REPOSITORY}/commits`);
  url.searchParams.set("sha", "main");
  url.searchParams.set("per_page", String(CHANGE_COUNT));

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

  const entries = await response.json();
  if (!Array.isArray(entries) || entries.length === 0) {
    throw new Error("GitHub returned no commits for main");
  }

  return entries.map(toLatestChange);
}

try {
  const latestChanges = await fetchLatestChanges();
  const source = `export type LatestChange = {\n  sha: string;\n  title: string;\n  url: string;\n  committedAt: string;\n  pullRequest: number | null;\n};\n\nconst latestChanges: readonly LatestChange[] = ${JSON.stringify(latestChanges, null, 2)};\n\nexport default latestChanges;\n`;
  await writeFile(outputUrl, source, "utf8");
  console.log(`Cached ${latestChanges.length} changes from GitHub.`);
} catch (error) {
  try {
    await access(outputUrl);
    const message = error instanceof Error ? error.message : String(error);
    console.warn(`Could not refresh GitHub history; using the cached copy. ${message}`);
  } catch {
    throw error;
  }
}
