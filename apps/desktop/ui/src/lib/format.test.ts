import { describe, expect, it } from "vitest";

import { formatFileSize } from "./format";

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
