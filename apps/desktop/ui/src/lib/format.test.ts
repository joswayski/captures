import { describe, expect, it } from "vitest";

import {
  fileSizeDeltaBaseline,
  formatFileSize,
  formatFileSizeDelta,
  formatUpdateSize,
} from "./format";

describe("formatFileSize", () => {
  it("uses compact decimal units", () => {
    expect(formatFileSize(999)).toBe("999 B");
    expect(formatFileSize(1_200)).toBe("1.2 KB");
    expect(formatFileSize(1_200_000)).toBe("1.2 MB");
    expect(formatFileSize(125_000_000)).toBe("125 MB");
  });

  it("handles missing or invalid byte counts", () => {
    expect(formatFileSize(0)).toBe("0 B");
    expect(formatFileSize(Number.NaN)).toBe("0 B");
  });
});

describe("formatUpdateSize", () => {
  it("never exposes raw byte counts in updater copy", () => {
    expect(formatUpdateSize(0)).toBe("0 KB");
    expect(formatUpdateSize(500)).toBe("0.5 KB");
    expect(formatUpdateSize(7_340_032)).toBe("7.3 MB");
    expect(formatUpdateSize(2_400_000_000)).toBe("2.4 GB");
  });
});

describe("fileSizeDeltaBaseline", () => {
  it("uses the flattened source image when present so compact files are not the original", () => {
    expect(fileSizeDeltaBaseline(188_000, 1_100_000)).toBe(1_100_000);
    expect(fileSizeDeltaBaseline(1_100_000, null)).toBe(1_100_000);
    expect(fileSizeDeltaBaseline(250_000, 0)).toBe(250_000);
  });
});

describe("formatFileSizeDelta", () => {
  it("reports smaller and larger estimates against the original file", () => {
    expect(formatFileSizeDelta(400_000, 1_000_000)).toEqual({
      percent: -60,
      label: "−60%",
    });
    expect(formatFileSizeDelta(1_250_000, 1_000_000)).toEqual({
      percent: 25,
      label: "+25%",
    });
  });

  it("hides unknown or unchanged estimates", () => {
    expect(formatFileSizeDelta(null, 1_000_000)).toBeNull();
    expect(formatFileSizeDelta(1_000_000, 1_000_000)).toBeNull();
    expect(formatFileSizeDelta(1_004_000, 1_000_000)).toBeNull();
    expect(formatFileSizeDelta(250_000, 0)).toBeNull();
  });
});
