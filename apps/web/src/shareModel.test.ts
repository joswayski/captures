import assert from "node:assert/strict";
import test from "node:test";
import { mayIndex, type SharePageData } from "./shareModel.ts";

const publicShare: SharePageData = {
  kind: "ready",
  share: {
    id: "x",
    visibility: "public",
    passwordRequired: false,
    expiresAt: null,
    mediaUrl: "/api/shares/x/media",
  },
};
test("only accessible public shares may index", () => {
  assert.equal(mayIndex(publicShare), true);
  assert.equal(
    mayIndex({
      kind: "ready",
      share: { ...publicShare.share, passwordRequired: true, mediaUrl: null },
    }),
    false,
  );
  assert.equal(
    mayIndex({
      kind: "ready",
      share: { ...publicShare.share, visibility: "unlisted" },
    }),
    false,
  );
  assert.equal(mayIndex({ kind: "missing" }), false);
});
