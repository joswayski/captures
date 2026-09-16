import assert from "node:assert/strict";
import test from "node:test";

import { isDependencyUpdateTitle } from "./dependencyUpdates.ts";

test("recognizes grouped and versioned dependency bumps", () => {
  assert.equal(isDependencyUpdateTitle("Bump @vitest/mocker and vitest (#511)"), true);
  assert.equal(isDependencyUpdateTitle("Bump js-yaml from 4.3.1 to 4.3.2 (#513)"), true);
});

test("keeps product changes that happen to mention updates", () => {
  assert.equal(isDependencyUpdateTitle("Fix false crash reports during Windows updates (#520)"), false);
});
