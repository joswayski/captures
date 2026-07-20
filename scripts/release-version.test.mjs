import assert from "node:assert/strict";
import test from "node:test";

import { nextReleaseVersion, releaseDate } from "./release-version.mjs";

test("uses the New York calendar date across UTC midnight", () => {
  assert.equal(releaseDate(new Date("2026-07-20T01:30:00Z")), "2026-07-19");
  assert.equal(releaseDate(new Date("2026-07-20T04:30:00Z")), "2026-07-20");
});

test("creates the first public and updater-safe versions for a date", () => {
  assert.deepEqual(nextReleaseVersion("2026-07-19", []), {
    date: "2026-07-19",
    revision: 1,
    displayVersion: "2026.07.19.1",
    tag: "v2026.07.19.1",
    appVersion: "2026.7.1901",
  });
});

test("increments the highest same-day revision and ignores unrelated tags", () => {
  assert.equal(
    nextReleaseVersion("2026-07-19", [
      "v2026.07.18.9",
      "v2026.07.19.2",
      "something-else",
      "v2026.07.19.1",
    ]).revision,
    3,
  );
});

test("maps the next day to a larger internal semver", () => {
  const previous = nextReleaseVersion("2026-07-19", ["v2026.07.19.1"]);
  const next = nextReleaseVersion("2026-07-20", []);
  assert.equal(previous.appVersion, "2026.7.1902");
  assert.equal(next.appVersion, "2026.7.2001");
});

test("rejects malformed matching tags and invalid dates", () => {
  assert.throws(
    () => nextReleaseVersion("2026-07-19", ["v2026.07.19.beta"]),
    /malformed Captures release tag/u,
  );
  assert.throws(() => nextReleaseVersion("2026-02-30", []), /not a real calendar date/u);
});

test("caps daily releases at 99", () => {
  assert.throws(
    () => nextReleaseVersion("2026-07-19", ["v2026.07.19.99"]),
    /already has 99 releases/u,
  );
});
