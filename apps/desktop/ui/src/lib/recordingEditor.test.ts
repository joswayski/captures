import {
  capturesTimestampStem,
  isHistoryRecoveryMediaPath,
  recordingEditedFileStem,
  recordingInitialOutputFormat,
  recordingSourceFormat,
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

describe("recordingSourceFormat", () => {
  it("keeps GIFs as GIF regardless of mime or path", () => {
    expect(recordingSourceFormat({
      kind: "gif",
      mime_type: "video/mp4",
      path: "/tmp/clip.mp4",
    })).toBe("gif");
  });

  it("uses the container mime type for opened WebM files", () => {
    expect(recordingSourceFormat({
      kind: "video",
      mime_type: "video/webm",
      path: "/Users/example/Movies/clip.webm",
      saved_path: "/Users/example/Movies/clip.webm",
    })).toBe("webm");
  });

  it("falls back to the path extension when mime is generic", () => {
    expect(recordingSourceFormat({
      kind: "video",
      mime_type: "video/mp4",
      path: "/Users/example/Movies/clip.webm",
      saved_path: "/Users/example/Movies/clip.webm",
    })).toBe("webm");
    expect(recordingSourceFormat({
      kind: "video",
      mime_type: "application/octet-stream",
      path: "/Users/example/Captures/Captures_clip.mp4",
    })).toBe("mp4");
  });
});

describe("recordingInitialOutputFormat", () => {
  it("keeps opened WebM as WebM when the video preference is MP4 so Save can overwrite", () => {
    expect(recordingInitialOutputFormat("webm", "mp4")).toBe("webm");
  });

  it("honors an explicit WebM or GIF video preference for MP4 recordings", () => {
    expect(recordingInitialOutputFormat("mp4", "webm")).toBe("webm");
    expect(recordingInitialOutputFormat("mp4", "gif")).toBe("gif");
  });

  it("never converts a GIF recording to the video preference", () => {
    expect(recordingInitialOutputFormat("gif", "webm")).toBe("gif");
  });
});
