import assert from "node:assert/strict";
import test from "node:test";

import { detectPreviewDownloadId } from "./detectPreviewDownload.ts";

test("detects macOS from client hints and classic user agents", () => {
  assert.equal(
    detectPreviewDownloadId({
      userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
      platform: "MacIntel",
      userAgentDataPlatform: "macOS",
      maxTouchPoints: 0,
    }),
    "macos",
  );
  assert.equal(
    detectPreviewDownloadId({
      userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15",
      platform: "MacIntel",
      userAgentDataPlatform: "",
      maxTouchPoints: 0,
    }),
    "macos",
  );
});

test("detects Windows and Linux, preferring .deb on Debian-family user agents", () => {
  assert.equal(
    detectPreviewDownloadId({
      userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
      platform: "Win32",
      userAgentDataPlatform: "Windows",
      maxTouchPoints: 0,
    }),
    "windows",
  );
  assert.equal(
    detectPreviewDownloadId({
      userAgent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
      platform: "Linux x86_64",
      userAgentDataPlatform: "Linux",
      maxTouchPoints: 0,
    }),
    "linux-appimage",
  );
  assert.equal(
    detectPreviewDownloadId({
      userAgent: "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:129.0) Gecko/20100101 Firefox/129.0",
      platform: "Linux x86_64",
      userAgentDataPlatform: "",
      maxTouchPoints: 0,
    }),
    "linux-deb",
  );
});

test("does not pick a desktop installer on phones, tablets, or unknown systems", () => {
  assert.equal(
    detectPreviewDownloadId({
      userAgent:
        "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
      platform: "iPhone",
      userAgentDataPlatform: "iOS",
      maxTouchPoints: 5,
    }),
    null,
  );
  assert.equal(
    detectPreviewDownloadId({
      userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15",
      platform: "MacIntel",
      userAgentDataPlatform: "macOS",
      maxTouchPoints: 5,
    }),
    null,
  );
  assert.equal(
    detectPreviewDownloadId({
      userAgent: "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36",
      platform: "Linux armv8l",
      userAgentDataPlatform: "Android",
      maxTouchPoints: 5,
    }),
    null,
  );
  assert.equal(
    detectPreviewDownloadId({
      userAgent: "Mozilla/5.0 (X11; CrOS x86_64) AppleWebKit/537.36",
      platform: "Linux x86_64",
      userAgentDataPlatform: "Chrome OS",
      maxTouchPoints: 0,
    }),
    null,
  );
});
