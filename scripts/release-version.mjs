import { appendFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

export const RELEASE_TIME_ZONE = "America/New_York";

export function releaseDate(now = new Date(), timeZone = RELEASE_TIME_ZONE) {
  const fields = Object.fromEntries(
    new Intl.DateTimeFormat("en-US", {
      timeZone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    })
      .formatToParts(now)
      .filter(({ type }) => type !== "literal")
      .map(({ type, value }) => [type, value]),
  );
  return `${fields.year}-${fields.month}-${fields.day}`;
}

export function nextReleaseVersion(date, tags) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/u.exec(date);
  if (!match) throw new Error(`release date must use YYYY-MM-DD, received ${date}`);
  const [, yearText, monthText, dayText] = match;
  const year = Number(yearText);
  const month = Number(monthText);
  const day = Number(dayText);
  const normalized = new Date(`${date}T12:00:00Z`);
  if (
    Number.isNaN(normalized.valueOf())
    || normalized.getUTCFullYear() !== year
    || normalized.getUTCMonth() + 1 !== month
    || normalized.getUTCDate() !== day
  ) {
    throw new Error(`release date is not a real calendar date: ${date}`);
  }

  const prefix = `v${yearText}.${monthText}.${dayText}.`;
  const revisions = [];
  for (const tag of tags) {
    if (!tag.startsWith(prefix)) continue;
    const suffix = tag.slice(prefix.length);
    if (!/^[1-9]\d*$/u.test(suffix)) {
      throw new Error(`malformed Captures release tag for ${date}: ${tag}`);
    }
    const revision = Number(suffix);
    if (!Number.isSafeInteger(revision) || revision > 99) {
      throw new Error(`Captures supports at most 99 releases per day; found ${tag}`);
    }
    revisions.push(revision);
  }

  const revision = Math.max(0, ...revisions) + 1;
  if (revision > 99) {
    throw new Error(`Captures already has 99 releases for ${date}`);
  }
  const displayVersion = `${yearText}.${monthText}.${dayText}.${revision}`;
  return {
    date,
    revision,
    displayVersion,
    tag: `v${displayVersion}`,
    appVersion: `${year}.${month}.${day * 100 + revision}`,
  };
}

function currentTags() {
  return execFileSync("git", ["tag", "--list"], { encoding: "utf8" })
    .split(/\r?\n/u)
    .filter(Boolean);
}

export function configuredReleaseDate(timestamp = process.env.CAPTURES_RELEASE_TIMESTAMP) {
  if (!timestamp) return releaseDate();

  const parsed = new Date(timestamp);
  if (Number.isNaN(parsed.valueOf())) {
    throw new Error(`CAPTURES_RELEASE_TIMESTAMP must be an ISO-8601 timestamp, received ${timestamp}`);
  }
  return releaseDate(parsed);
}

function main() {
  const version = nextReleaseVersion(configuredReleaseDate(), currentTags());
  const output = process.env.GITHUB_OUTPUT;
  if (output) {
    appendFileSync(
      output,
      [
        `release_date=${version.date}`,
        `revision=${version.revision}`,
        `display_version=${version.displayVersion}`,
        `tag=${version.tag}`,
        `app_version=${version.appVersion}`,
        "",
      ].join("\n"),
    );
  }
  process.stdout.write(`${JSON.stringify(version)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
