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
      "Improve capture parity",
      "Fix the region selector",
    ]);
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
      { version: "2026.8.2705", displayVersion: "2026.08.27.5", items: ["Fix the update notice"] },
      { version: "2026.8.2704", displayVersion: "2026.08.27.4", items: ["Fix capture menu switching"] },
    ]);

    expect(stackedReleaseNotes([], sampleNotes, "2026.08.27.5")).toEqual([{
      version: "",
      displayVersion: "2026.08.27.5",
      items: ["Improve capture parity", "Fix the region selector"],
    }]);
  });
});
