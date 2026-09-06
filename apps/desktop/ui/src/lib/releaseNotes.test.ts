import { releaseNoteItems, stackedReleaseNotes } from "./releaseNotes";

const sampleNotes = [
  "> [!WARNING]",
  "> This Preview is functional, but experimental.",
  "",
  "## What's Changed",
  "* Improve capture parity by @joswayski in https://github.com/joswayski/captures/pull/249",
  "* **Fix** the `[region](https://example.com)` selector",
  "* @devin-ai-integration[bot] made their first contribution in https://github.com/joswayski/captures/pull/297",
  "",
  "## New Contributors",
  "* @someone made their first contribution in https://github.com/joswayski/captures/pull/1",
  "",
  "**Full Changelog**: https://github.com/joswayski/captures/compare/old...new",
].join("\n");

describe("releaseNoteItems", () => {
  it("keeps changes while removing Preview boilerplate and GitHub Markdown", () => {
    expect(releaseNoteItems(sampleNotes)).toEqual([
      {
        text: "Improve capture parity",
        pullRequest: {
          number: 249,
          url: "https://github.com/joswayski/captures/pull/249",
        },
      },
      { text: "Fix the region selector", pullRequest: null },
    ]);
    expect(releaseNoteItems(
      "* Fix Linux startup crash by @devin-ai-integration[bot] in https://github.com/joswayski/captures/pull/297\n* @devin-ai-integration[bot] made their first contribution in https://github.com/joswayski/captures/pull/297",
    )).toEqual([{
      text: "Fix Linux startup crash",
      pullRequest: {
        number: 297,
        url: "https://github.com/joswayski/captures/pull/297",
      },
    }]);
  });

  it("links squash-merge titles that only include a PR number", () => {
    expect(releaseNoteItems("* Keep the update notice capturable (#452)")).toEqual([{
      text: "Keep the update notice capturable",
      pullRequest: {
        number: 452,
        url: "https://github.com/joswayski/captures/pull/452",
      },
    }]);
  });
});

describe("stackedReleaseNotes", () => {
  it("keeps each skipped Preview's notes and falls back to the latest body", () => {
    expect(stackedReleaseNotes(
      [
        {
          version: "2026.8.2705",
          display_version: "2026.08.27.5",
          notes: "* Fix the update notice",
        },
        {
          version: "2026.8.2704",
          display_version: "2026.08.27.4",
          notes: "* Fix capture menu switching",
        },
      ],
      "* Only the latest notes",
      "2026.08.27.5",
    )).toEqual([
      {
        version: "2026.8.2705",
        displayVersion: "2026.08.27.5",
        items: [{ text: "Fix the update notice", pullRequest: null }],
      },
      {
        version: "2026.8.2704",
        displayVersion: "2026.08.27.4",
        items: [{ text: "Fix capture menu switching", pullRequest: null }],
      },
    ]);

    expect(stackedReleaseNotes([], sampleNotes, "2026.08.27.5")).toEqual([{
      version: "",
      displayVersion: "2026.08.27.5",
      items: [
        {
          text: "Improve capture parity",
          pullRequest: {
            number: 249,
            url: "https://github.com/joswayski/captures/pull/249",
          },
        },
        { text: "Fix the region selector", pullRequest: null },
      ],
    }]);
  });
});
