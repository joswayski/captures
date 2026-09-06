const AUTHOR_PR_SUFFIX =
  /\s+by\s+@[\w-]+(?:\[bot\])?\s+in\s+https?:\/\/\S+\s*$/iu;
const GITHUB_PULL_URL =
  /https:\/\/(?:www\.)?github\.com\/([\w.-]+)\/([\w.-]+)\/pull\/(\d+)/iu;
const TRAILING_PR_NUMBER = /\s+\(#(\d+)\)\s*$/u;
const CAPTURES_GITHUB_OWNER = "joswayski";
const CAPTURES_GITHUB_REPO = "captures";

export interface ReleaseNotePullRequest {
  number: number;
  url: string;
}

export interface ReleaseNoteItem {
  text: string;
  pullRequest: ReleaseNotePullRequest | null;
}

function plainText(markdown: string) {
  return markdown
    .replace(/\[([^\]]+)\]\([^\s)]+(?:\s+"[^"]*")?\)/gu, "$1")
    .replace(/<([^>]+)>/gu, "$1")
    .replace(/[*_~`]+/gu, "")
    .replace(AUTHOR_PR_SUFFIX, "")
    .replace(TRAILING_PR_NUMBER, "")
    .trim();
}

function pullRequest(owner: string, repo: string, number: string): ReleaseNotePullRequest {
  return {
    number: Number(number),
    url: `https://github.com/${owner}/${repo}/pull/${number}`,
  };
}

function pullRequestFromLine(line: string): ReleaseNotePullRequest | null {
  const fromUrl = line.match(GITHUB_PULL_URL);
  if (fromUrl?.[1] && fromUrl[2] && fromUrl[3]) {
    return pullRequest(fromUrl[1], fromUrl[2], fromUrl[3]);
  }

  const trailing = line.match(TRAILING_PR_NUMBER);
  if (!trailing?.[1]) return null;
  return pullRequest(CAPTURES_GITHUB_OWNER, CAPTURES_GITHUB_REPO, trailing[1]);
}

/** GitHub auto-appends these under New Contributors; they are not product changes. */
function isFirstContributionLine(text: string) {
  return /\bmade their first contribution\b/iu.test(text);
}

/** Turn GitHub's generated release Markdown into concise, safe toast copy. */
export function releaseNoteItems(markdown: string): ReleaseNoteItem[] {
  const items: ReleaseNoteItem[] = [];
  let skippingAlert = false;

  for (const sourceLine of markdown.split(/\r?\n/u)) {
    const line = sourceLine.trim();
    if (/^>\s*\[![A-Z]+\]/u.test(line)) {
      skippingAlert = true;
      continue;
    }
    if (skippingAlert && line.startsWith(">")) continue;
    if (line === "") {
      skippingAlert = false;
      continue;
    }
    skippingAlert = false;

    if (/^#{1,6}\s+/u.test(line) || /^\*{0,2}Full Changelog\*{0,2}\s*:/iu.test(line)) {
      continue;
    }

    const body = line.replace(/^(?:[-*+]\s+|\d+[.)]\s+)/u, "").replace(/^>\s?/u, "");
    const text = plainText(body);
    if (!text || isFirstContributionLine(text) || isFirstContributionLine(body)) continue;
    items.push({ text, pullRequest: pullRequestFromLine(body) });
  }

  return items;
}

export interface ReleaseNoteGroup {
  version: string;
  displayVersion: string;
  items: ReleaseNoteItem[];
}

export function stackedReleaseNotes(
  changelog: Array<{ version: string; display_version: string; notes: string | null }> | null | undefined,
  fallbackNotes: string | null | undefined,
  fallbackDisplayVersion: string,
): ReleaseNoteGroup[] {
  if (changelog && changelog.length > 0) {
    return changelog.map((entry) => ({
      version: entry.version,
      displayVersion: entry.display_version,
      items: entry.notes ? releaseNoteItems(entry.notes) : [],
    }));
  }
  if (!fallbackNotes) return [];
  return [{
    version: "",
    displayVersion: fallbackDisplayVersion,
    items: releaseNoteItems(fallbackNotes),
  }];
}
