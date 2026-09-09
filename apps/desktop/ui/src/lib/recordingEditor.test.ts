import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  capturesTimestampStem,
  isHistoryRecoveryMediaPath,
  recordingEditedFileStem,
  recordingUserFacingDefaults,
  timelineHandleTrim,
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
  it("keeps handle centers on the filmstrip via token inset, not overflowed 0% / 100%", () => {
    expect(timelineRatio(0, 8_750)).toBe(0);
    expect(timelineRatio(8_750, 8_750)).toBe(1);
    expect(timelineHandleTrim(0, 8_750)).toBe("0");
    expect(timelineHandleTrim(4_375, 8_750)).toBe("0.5");
    expect(timelineHandleTrim(8_750, 8_750)).toBe("1");
    expect(editorVideoStyles).toMatch(/--timeline-inset:\s*var\(--s-2\)/);
    expect(editorVideoStyles).toMatch(/\.timeline-filmstrip\s*\{[^}]*inset:\s*var\(--timeline-inset\)/s);
    expect(editorVideoStyles).toMatch(
      /left:\s*calc\(\s*var\(--timeline-inset\)\s*\+\s*\(var\(--trim\) \* \(100% - \(var\(--timeline-inset\) \* 2\)\)\)/s,
    );
  });

  it("maps a pointer on the track to a clamped timeline time", () => {
    expect(timelineTimeAtClientX(500, { left: 0, width: 1_000 }, 8_750)).toBe(4_375);
    expect(timelineTimeAtClientX(-20, { left: 0, width: 1_000 }, 8_750)).toBe(0);
    expect(timelineTimeAtClientX(2_000, { left: 0, width: 1_000 }, 8_750)).toBe(8_750);
  });

  it("applies large in-bounds steps and ignores coordinates that are not near the track", () => {
    const start = {
      startTime: 2_000,
      startX: 228.57142857142858,
      lastX: 228.57142857142858,
      trackLeft: 0,
      trackWidth: 1_000,
      duration: 8_750,
      min: 0,
      max: 6_749,
    };
    expect(timelineTimeFromPointerDrag({ ...start, clientX: start.startX + 50 }).time)
      .toBeCloseTo(2_437.5, 5);
    expect(timelineTimeFromPointerDrag({ ...start, clientX: start.startX + 500 }).time)
      .toBeCloseTo(6_375, 5);

    const glitch = timelineTimeFromPointerDrag({ ...start, clientX: 9_000 });
    expect(glitch.time).toBe(2_000);
    expect(glitch.lastX).toBe(start.lastX);
    expect(glitch.startX).toBe(start.startX);
    expect(timelineTimeFromPointerDrag({
      ...glitch,
      clientX: 9_050,
      trackLeft: 0,
      trackWidth: 1_000,
      duration: 8_750,
      min: 0,
      max: 6_749,
    }).time).toBe(2_000);
    expect(timelineTimeFromPointerDrag({
      ...start,
      ...glitch,
      clientX: start.startX + 50,
    }).time).toBeCloseTo(2_437.5, 5);
  });

  it("clips horizontal overflow so trim handles cannot scroll the editor offscreen", () => {
    expect(editorVideoStyles).toMatch(/\.recording-editor\s*\{[^}]*overflow-x:\s*hidden/s);
    expect(editorVideoStyles).toMatch(/\.timeline-filmstrip i\s*\{[^}]*flex:\s*1 1 0/s);
    expect(editorVideoStyles).toMatch(/\.timeline-trim-end > span\s*\{[^}]*right:\s*0/s);
  });
});
