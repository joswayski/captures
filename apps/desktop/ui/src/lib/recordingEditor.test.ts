import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  capturesTimestampStem,
  isHistoryRecoveryMediaPath,
  recordingEditedFileStem,
  recordingUserFacingDefaults,
  TIMELINE_HANDLE_INSET_PX,
  timelineHandleLeft,
  timelineRatio,
  timelineTimeAtClientX,
  timelineTimeFromPointerDrag,
} from "./recordingEditor";

const editorVideoStyles = readFileSync(
  resolve(process.cwd(), "ui/src/styles/editor-video.css"),
  "utf8",
);

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

describe("timeline trim handle geometry", () => {
  it("keeps handle centers on the filmstrip inset instead of overflowing 0% / 100%", () => {
    expect(timelineRatio(0, 8_750)).toBe(0);
    expect(timelineRatio(8_750, 8_750)).toBe(1);
    expect(timelineHandleLeft(0, 8_750)).toBe(`calc(0% + ${TIMELINE_HANDLE_INSET_PX}px)`);
    expect(timelineHandleLeft(4_375, 8_750)).toBe("50%");
    expect(timelineHandleLeft(8_750, 8_750)).toBe(`calc(100% - ${TIMELINE_HANDLE_INSET_PX}px)`);
  });

  it("maps a pointer on the track to a clamped timeline time", () => {
    expect(timelineTimeAtClientX(500, { left: 0, width: 1_000 }, 8_750)).toBe(4_375);
    expect(timelineTimeAtClientX(-20, { left: 0, width: 1_000 }, 8_750)).toBe(0);
    expect(timelineTimeAtClientX(2_000, { left: 0, width: 1_000 }, 8_750)).toBe(8_750);
  });

  it("drags from the press origin so a captured-pointer jump cannot slam to the end", () => {
    const start = {
      startTime: 2_000,
      startX: 228.57142857142858,
      lastX: 228.57142857142858,
      trackWidth: 1_000,
      duration: 8_750,
      min: 0,
      max: 6_749,
    };
    expect(timelineTimeFromPointerDrag({ ...start, clientX: start.startX + 50 }).time)
      .toBeCloseTo(2_437.5, 5);

    const glitch = timelineTimeFromPointerDrag({ ...start, clientX: 9_000 });
    expect(glitch.time).toBe(2_000);
    expect(glitch.startX).toBe(9_000);
    expect(glitch.startTime).toBe(2_000);

    expect(timelineTimeFromPointerDrag({
      ...glitch,
      clientX: 9_050,
      trackWidth: 1_000,
      duration: 8_750,
      min: 0,
      max: 6_749,
    }).time).toBeCloseTo(2_437.5, 5);
  });

  it("clips horizontal overflow so trim handles cannot scroll the editor offscreen", () => {
    expect(editorVideoStyles).toMatch(/\.recording-editor\s*\{[^}]*overflow-x:\s*hidden/s);
    expect(editorVideoStyles).toMatch(/\.timeline-filmstrip i\s*\{[^}]*flex:\s*1 1 0/s);
    expect(editorVideoStyles).toMatch(/\.timeline-trim-end > span\s*\{[^}]*right:\s*0/s);
  });
});
