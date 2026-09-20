import assert from "node:assert/strict";
import test from "node:test";
import {
  MAX_UPLOAD_BYTES,
  formatBytes,
  validateShare,
  validateUpload,
} from "./accountModel.ts";

test("upload validation limits type, size, and empty files", () => {
  assert.equal(validateUpload({ type: "image/png", size: 42 }), null);
  assert.equal(validateUpload({ type: "image/png", size: MAX_UPLOAD_BYTES }), null);
  assert.match(validateUpload({ type: "image/gif", size: 42 })!, /PNG/);
  assert.match(
    validateUpload({ type: "image/jpeg", size: MAX_UPLOAD_BYTES + 1 })!,
    /20 MiB/,
  );
  assert.match(validateUpload({ type: "image/webp", size: 0 })!, /empty/);
});

test("share validation checks password and future expiry", () => {
  assert.match(validateShare("short", "")!, /8/);
  assert.match(validateShare("", "2000-01-01T00:00")!, /future/);
  assert.match(validateShare("", "not a date")!, /future/);
  assert.equal(validateShare("eight-ok", "2999-01-01T00:00"), null);
  assert.equal(formatBytes(2 * 1024 * 1024), "2.0 MiB");
});
