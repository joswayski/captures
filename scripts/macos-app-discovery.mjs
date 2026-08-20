import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";

export const SPOTLIGHT_EXCLUSION_MARKER = ".metadata_never_index";
export const HIDDEN_BUNDLE_DIRECTORY_NAME = "macos.noindex";
export const LSREGISTER_PATH =
  "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

function defaultRun(command, args) {
  return spawnSync(command, args, { encoding: "utf8" });
}

export function spotlightExclusionMarkerPath(targetDirectory) {
  return join(targetDirectory, SPOTLIGHT_EXCLUSION_MARKER);
}

export function hiddenCheckoutAppPath(builtApp) {
  return join(dirname(dirname(builtApp)), HIDDEN_BUNDLE_DIRECTORY_NAME, basename(builtApp));
}

export function hideCheckoutMacAppFromLaunchers({
  targetDirectory,
  builtApp,
  applicationsApp,
  relocateCheckoutApp = true,
  log = console.log,
  io = {
    existsSync,
    mkdirSync,
    renameSync,
    rmSync,
    writeFileSync,
    run: defaultRun,
  },
}) {
  io.mkdirSync(targetDirectory, { recursive: true });
  io.writeFileSync(spotlightExclusionMarkerPath(targetDirectory), "");

  if (io.existsSync(builtApp) && builtApp !== applicationsApp) {
    io.run(LSREGISTER_PATH, ["-u", builtApp]);
    if (relocateCheckoutApp) {
      const hiddenApp = hiddenCheckoutAppPath(builtApp);
      io.mkdirSync(dirname(hiddenApp), { recursive: true });
      if (io.existsSync(hiddenApp)) {
        io.rmSync(hiddenApp, { recursive: true, force: true });
      }
      io.renameSync(builtApp, hiddenApp);
      io.run(LSREGISTER_PATH, ["-u", hiddenApp]);
      log(
        `Moved the checkout copy to ${hiddenApp} so Spotlight and other app launchers keep /Applications/${basename(builtApp)}.`,
      );
    }
  }

  if (io.existsSync(applicationsApp)) {
    io.run(LSREGISTER_PATH, ["-f", applicationsApp]);
  }
}
