import {
  capturesTimestampStem,
  isHistoryRecoveryMediaPath,
  recordingEditedFileStem,
  recordingUserFacingDefaults,
} from "./recordingEditor";

describe("recordingUserFacingDefaults", () => {
  it("prefers a permanent Captures-folder save over private recovery media", () => {
    expect(
      recordingUserFacingDefaults({
        path: "/Users/example/Library/Application Support/Captures/history/abc/media.mp4",
        savedPath: "/Users/example/Captures/Captures_clip.mp4",
        createdAt: "2026-07-26T16:45:01.250Z",
        outputDirectory: "/Users/example/Captures",
      }),
    ).toEqual({
      directory: "/Users/example/Captures",
      stem: "Captures_clip",
    });
  });

  it("defaults history-only recordings to the Captures folder and a timestamp name", () => {
    const defaults = recordingUserFacingDefaults({
      path: "/Users/example/Library/Application Support/Captures/history/abc/media.mp4",
      savedPath: null,
      createdAt: "2026-07-26T16:45:01.250Z",
      outputDirectory: "/Users/example/Captures",
    });
    expect(defaults.directory).toBe("/Users/example/Captures");
    expect(defaults.stem).toMatch(/^Captures_\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}_\d{3}$/);
    expect(defaults.stem).not.toBe("media");
  });

  it("keeps legacy Captures-folder paths when there is no separate saved_path", () => {
    expect(
      recordingUserFacingDefaults({
        path: "/Users/example/Captures/Captures_1140x692.mp4",
        savedPath: null,
        createdAt: "2026-07-26T16:45:01Z",
        outputDirectory: "/Users/example/Captures",
      }),
    ).toEqual({
      directory: "/Users/example/Captures",
      stem: "Captures_1140x692",
    });
  });
});

describe("history recovery media detection", () => {
  it("recognizes private recovery basenames", () => {
    expect(isHistoryRecoveryMediaPath("/tmp/history/id/media.mp4")).toBe(true);
    expect(isHistoryRecoveryMediaPath("/tmp/history/id/media.gif")).toBe(true);
    expect(isHistoryRecoveryMediaPath("/tmp/Captures/Captures_clip.mp4")).toBe(false);
  });
});

describe("capturesTimestampStem", () => {
  it("formats local timestamps like desktop capture names", () => {
    const created = new Date(2026, 6, 26, 12, 34, 56, 78);
    expect(capturesTimestampStem(created.toISOString(), created))
      .toBe("Captures_2026-07-26_12-34-56_078");
  });
});

describe("recordingEditedFileStem", () => {
  it("appends -edited once", () => {
    expect(recordingEditedFileStem("Captures_clip")).toBe("Captures_clip-edited");
    expect(recordingEditedFileStem("Captures_clip-edited")).toBe("Captures_clip-edited");
    expect(recordingEditedFileStem("Captures_clip-copy")).toBe("Captures_clip-copy");
  });
});
