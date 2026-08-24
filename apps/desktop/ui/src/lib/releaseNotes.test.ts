import { releaseNoteItems } from "./releaseNotes";

describe("releaseNoteItems", () => {
  it("keeps changes while removing Preview boilerplate and GitHub Markdown", () => {
    expect(releaseNoteItems([
      "> [!WARNING]",
      "> This Preview is functional, but experimental.",
      "",
      "## What's Changed",
      "* Improve capture parity by @joswayski in https://github.com/joswayski/captures/pull/249",
      "* **Fix** the `[region](https://example.com)` selector",
      "",
      "**Full Changelog**: https://github.com/joswayski/captures/compare/old...new",
    ].join("\n"))).toEqual([
      "Improve capture parity",
      "Fix the region selector",
    ]);
  });
});
