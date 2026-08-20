import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join } from "node:path";

import {
  HIDDEN_BUNDLE_DIRECTORY_NAME,
  LSREGISTER_PATH,
  hiddenCheckoutAppPath,
  hideCheckoutMacAppFromLaunchers,
  spotlightExclusionMarkerPath,
} from "./macos-app-discovery.mjs";

function fakeIo(existingPaths) {
  const files = new Set(existingPaths);
  const directories = new Set(
    [...existingPaths].flatMap((path) => {
      const parts = [];
      let current = dirname(path);
      while (current && current !== dirname(current)) {
        parts.push(current);
        current = dirname(current);
      }
      return parts;
    }),
  );
  const writes = [];
  const runs = [];
  const renames = [];
  const removals = [];

  return {
    files,
    writes,
    runs,
    renames,
    removals,
    existsSync(path) {
      return files.has(path) || directories.has(path);
    },
    mkdirSync(path) {
      directories.add(path);
    },
    writeFileSync(path, contents) {
      writes.push({ path, contents });
      files.add(path);
    },
    renameSync(from, to) {
      files.delete(from);
      files.add(to);
      directories.add(dirname(to));
      renames.push({ from, to });
    },
    rmSync(path) {
      files.delete(path);
      removals.push(path);
    },
    run(command, args) {
      runs.push({ command, args });
      return { status: 0 };
    },
  };
}

test("derives Spotlight and hidden-bundle paths from the checkout layout", () => {
  const targetDirectory = join("/repo", "target");
  const builtApp = join(targetDirectory, "release", "bundle", "macos", "Captures.app");
  assert.equal(spotlightExclusionMarkerPath(targetDirectory), join(targetDirectory, ".metadata_never_index"));
  assert.equal(
    hiddenCheckoutAppPath(builtApp),
    join(targetDirectory, "release", "bundle", HIDDEN_BUNDLE_DIRECTORY_NAME, "Captures.app"),
  );
});

test("hides an installed checkout copy from Spotlight and seeds /Applications", () => {
  const builtApp = "/repo/target/release/bundle/macos/Captures.app";
  const applicationsApp = "/Applications/Captures.app";
  const hiddenApp = hiddenCheckoutAppPath(builtApp);
  const messages = [];
  const io = fakeIo([builtApp, applicationsApp]);

  hideCheckoutMacAppFromLaunchers({
    targetDirectory: "/repo/target",
    builtApp,
    applicationsApp,
    log: (message) => messages.push(message),
    io,
  });

  assert.deepEqual(io.writes, [{ path: "/repo/target/.metadata_never_index", contents: "" }]);
  assert.deepEqual(io.renames, [{ from: builtApp, to: hiddenApp }]);
  assert.deepEqual(io.runs, [
    { command: LSREGISTER_PATH, args: ["-u", builtApp] },
    { command: LSREGISTER_PATH, args: ["-u", hiddenApp] },
    { command: LSREGISTER_PATH, args: ["-f", applicationsApp] },
  ]);
  assert.ok(messages[0].includes(hiddenApp));
});

test("replaces a previous hidden checkout copy before moving", () => {
  const builtApp = "/repo/target/release/bundle/macos/Captures.app";
  const applicationsApp = "/Applications/Captures.app";
  const hiddenApp = hiddenCheckoutAppPath(builtApp);
  const io = fakeIo([builtApp, applicationsApp, hiddenApp]);

  hideCheckoutMacAppFromLaunchers({
    targetDirectory: "/repo/target",
    builtApp,
    applicationsApp,
    log() {},
    io,
  });

  assert.deepEqual(io.removals, [hiddenApp]);
  assert.deepEqual(io.renames, [{ from: builtApp, to: hiddenApp }]);
});

test("keeps a skip-install checkout bundle in place after unregistering it", () => {
  const builtApp = "/repo/target/release/bundle/macos/Captures.app";
  const applicationsApp = "/Applications/Captures.app";
  const io = fakeIo([builtApp]);

  hideCheckoutMacAppFromLaunchers({
    targetDirectory: "/repo/target",
    builtApp,
    applicationsApp,
    relocateCheckoutApp: false,
    log() {},
    io,
  });

  assert.deepEqual(io.renames, []);
  assert.deepEqual(io.runs, [{ command: LSREGISTER_PATH, args: ["-u", builtApp] }]);
});

test("still writes the Spotlight marker when no checkout app exists", () => {
  const io = fakeIo(["/Applications/Captures.app"]);

  hideCheckoutMacAppFromLaunchers({
    targetDirectory: "/repo/target",
    builtApp: "/repo/target/release/bundle/macos/Captures.app",
    applicationsApp: "/Applications/Captures.app",
    log() {},
    io,
  });

  assert.deepEqual(io.writes, [{ path: "/repo/target/.metadata_never_index", contents: "" }]);
  assert.deepEqual(io.renames, []);
  assert.deepEqual(io.runs, [{ command: LSREGISTER_PATH, args: ["-f", "/Applications/Captures.app"] }]);
});
