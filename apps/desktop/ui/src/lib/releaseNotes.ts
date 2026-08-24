function plainText(markdown: string) {
  return markdown
    .replace(/\[([^\]]+)\]\([^\s)]+(?:\s+"[^"]*")?\)/gu, "$1")
    .replace(/<([^>]+)>/gu, "$1")
    .replace(/[*_~`]+/gu, "")
    .replace(/\s+by\s+@[\w-]+\s+in\s+https?:\/\/\S+\s*$/iu, "")
    .trim();
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
    if (text) items.push(text);
  }

  return items;
}
