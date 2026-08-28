import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { PRODUCT_SHOTS, galleryFrameAspectRatio } from "./productShots.ts";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const imagesDir = join(repoRoot, "docs/images");

test("website product shots cover the README stills with unique files", () => {
  assert.deepEqual(
    PRODUCT_SHOTS.map((shot) => shot.id),
    [
      "capture-selection",
      "screenshot-editor",
      "capture-controls",
      "video-editor",
      "preferences",
    ],
  );
  assert.equal(PRODUCT_SHOTS.length, 5);
  const files = new Set(PRODUCT_SHOTS.map((shot) => shot.file));
  const ids = new Set(PRODUCT_SHOTS.map((shot) => shot.id));
  assert.equal(files.size, PRODUCT_SHOTS.length);
  assert.equal(ids.size, PRODUCT_SHOTS.length);
  for (const shot of PRODUCT_SHOTS) {
    assert.match(shot.file, /^[a-z0-9-]+\.jpg$/u);
    assert.ok(shot.title.trim());
    assert.ok(shot.description.trim());
    assert.ok(shot.alt.length > 20);
    assert.ok(shot.width > 0);
    assert.ok(shot.height > 0);
  }
});

test("gallery frame aspect is as tall as every still so captions do not jump", () => {
  const ratio = galleryFrameAspectRatio();
  assert.ok(ratio > 0);
  for (const shot of PRODUCT_SHOTS) {
    assert.ok(shot.height / shot.width <= ratio + Number.EPSILON);
  }
});

test("each product shot file exists and matches the declared JPEG size", () => {
  for (const shot of PRODUCT_SHOTS) {
    const path = join(imagesDir, shot.file);
    const bytes = readFileSync(path);
    const size = jpegSize(bytes);
    assert.deepEqual(
      size,
      { width: shot.width, height: shot.height },
      `${shot.file} dimensions`,
    );
  }
});

function jpegSize(bytes: Buffer) {
  if (bytes[0] !== 0xff || bytes[1] !== 0xd8) {
    throw new Error("Not a JPEG");
  }

  let offset = 2;
  while (offset + 8 < bytes.length) {
    if (bytes[offset] !== 0xff) {
      throw new Error("Invalid JPEG marker");
    }
    const marker = bytes[offset + 1];
    if (marker === 0xd8 || marker === 0xd9 || (marker >= 0xd0 && marker <= 0xd7)) {
      offset += 2;
      continue;
    }
    const length = (bytes[offset + 2] << 8) | bytes[offset + 3];
    if (isStartOfFrame(marker)) {
      return {
        height: (bytes[offset + 5] << 8) | bytes[offset + 6],
        width: (bytes[offset + 7] << 8) | bytes[offset + 8],
      };
    }
    offset += 2 + length;
  }

  throw new Error("JPEG is missing a start-of-frame marker");
}

function isStartOfFrame(marker: number) {
  return (
    (marker >= 0xc0 && marker <= 0xc3)
    || (marker >= 0xc5 && marker <= 0xc7)
    || (marker >= 0xc9 && marker <= 0xcb)
    || (marker >= 0xcd && marker <= 0xcf)
  );
}
