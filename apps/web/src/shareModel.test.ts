import assert from "node:assert/strict";
import test from "node:test";
import { mayIndex, shareMediaKind, type SharePageData } from "./shareModel.ts";

const share: SharePageData = {
  kind: "ready",
  share: {
    id: "Ab_cdEF012-3",
    name: "capture.gif",
    contentType: "image/gif",
    byteSize: 10,
    passwordRequired: false,
    expiresAt: null,
    mediaUrl: "/media",
  },
};

test("all share states remain unlisted from search", () => {
  assert.equal(mayIndex(share), false);
  assert.equal(mayIndex({ kind: "missing" }), false);
  assert.equal(mayIndex({ kind: "unavailable" }), false);
});

test("viewer renders original raster and video but downloads unsafe or unknown media", () => {
  assert.equal(shareMediaKind("image/gif"), "image");
  assert.equal(shareMediaKind("video/webm"), "video");
  assert.equal(shareMediaKind("image/svg+xml"), "download");
  assert.equal(shareMediaKind("text/html"), "download");
});
