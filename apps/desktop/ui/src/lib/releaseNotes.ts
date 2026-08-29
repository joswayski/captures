function plainText(markdown: string) {
  return markdown
    .replace(/\[([^\]]+)\]\([^\s)]+(?:\s+"[^"]*")?\)/gu, "$1")
    .replace(/<([^>]+)>/gu, "$1")
    .replace(/[*_~`]+/gu, "")
    .replace(/\s+by\s+@[\w-]+(?:\[bot\])?\s+in\s+https?:\/\/\S+\s*$/iu, "")
    .trim();
}

/** GitHub auto-appends these under New Contributors; they are not product changes. */
function isFirstContributionLine(text: string) {
  return /\bmade their first contribution\b/iu.test(text);
}

/** Turn GitHub's generated release Markdown into concise, safe toast copy. */
export function releaseNoteItems(markdown: string): string[] {
  const items: string[] = [];
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

    const text = plainText(line.replace(/^(?:[-*+]\s+|\d+[.)]\s+)/u, "").replace(/^>\s?/u, ""));
    if (!text || isFirstContributionLine(text)) continue;
    items.push(text);
  }

  return items;
}

export interface ReleaseNoteGroup {
  version: string;
  displayVersion: string;
  items: string[];
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
