import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const APP_NAME = "Captures";
const BINARY_NAME = "captures";
const APP_BUNDLE = `${APP_NAME}.app`;
const APPLICATIONS_APP = join("/Applications", APP_BUNDLE);
const BUNDLE_ROOT = join(ROOT, "target/release/bundle");
const BUILT_APP = join(ROOT, "target/release/bundle/macos", APP_BUNDLE);
const BUILT_WINDOWS_EXE = join(ROOT, "target/release", `${BINARY_NAME}.exe`);

const environment = { ...process.env };
const isMac = process.platform === "darwin";
const isWindows = process.platform === "win32";
const skipInstall = process.env.CAPTURES_SKIP_INSTALL === "1";
// Permission reset is OPT-IN. Default used to wipe Screen Recording on every
// build (via tccutil), which with ad-hoc signing left captures completely dead
// until the user re-approved + restarted — often looking like "overlays never show".
const resetPermissions = process.env.CAPTURES_RESET_PERMISSIONS === "1";
const openAfterInstall = process.env.CAPTURES_OPEN_AFTER_INSTALL !== "0";

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    encoding: "utf8",
    stdio: options.stdio ?? "pipe",
    env: options.env ?? process.env,
    cwd: options.cwd,
  });
}

function log(message) {
  console.log(message);
}

function commandError(command, result) {
  const detail = result.error?.message || result.stderr?.trim();
  return new Error(detail ? `${command} failed: ${detail}` : `${command} failed with status ${result.status}`);
}

function runChecked(command, args, options = {}) {
  const result = run(command, args, options);
  if (result.error || result.status !== 0) {
    throw commandError(command, result);
  }
  return result;
}

function npmCliPath() {
  const candidates = [
    environment.npm_execpath,
    join(dirname(process.execPath), "node_modules/npm/bin/npm-cli.js"),
  ];
  return candidates.find((candidate) => candidate && existsSync(candidate));
}

function configureMacBuildEnvironment() {
  if (!isMac) return;

  if (!environment.SDKROOT) {
    const sdk = run("/usr/bin/xcrun", ["--sdk", "macosx", "--show-sdk-path"], {
      env: environment,
    });
    const defaultSdk = sdk.status === 0 ? sdk.stdout?.trim() : null;
    if (defaultSdk) {
      environment.SDKROOT = defaultSdk;
      log(`Using the active macOS SDK at ${defaultSdk}.`);
    }
  }

  const moduleCacheRoot = join(ROOT, "target", "module-cache");
  const clangModuleCache = join(moduleCacheRoot, "clang");
  const swiftModuleCache = join(moduleCacheRoot, "swift");
  mkdirSync(clangModuleCache, { recursive: true });
  mkdirSync(swiftModuleCache, { recursive: true });
  environment.CLANG_MODULE_CACHE_PATH ??= clangModuleCache;
  environment.SWIFTPM_MODULECACHE_OVERRIDE ??= swiftModuleCache;
}

function processIsRunning(name) {
  return run("/usr/bin/pgrep", ["-x", name]).status === 0;
}

function mountedBuildDevices(hdiutilOutput) {
  const bundlePrefix = `${BUNDLE_ROOT}/`;
  const devices = new Set();
  for (const image of hdiutilOutput.split(/\n={10,}\n/u)) {
    const imagePath = image.match(/^image-path\s+:\s+(.+)$/mu)?.[1];
    const device = image.match(/^(\/dev\/disk\d+)\s/mu)?.[1];
    if (imagePath?.startsWith(bundlePrefix) && device) devices.add(device);
  }
  return [...devices];
}

function detachMountedBuildImages() {
  const info = run("/usr/bin/hdiutil", ["info"]);
  if (info.error || info.status !== 0) {
    console.warn("Could not inspect mounted disk images; continuing with the build.");
    return;
  }

  const devices = mountedBuildDevices(info.stdout ?? "");
  if (devices.length === 0) return;

  log(`Unmounting ${devices.length} stale Captures build disk image${devices.length === 1 ? "" : "s"}…`);
  for (const device of devices) {
    const detached = run("/usr/bin/hdiutil", ["detach", device]);
    if (detached.status === 0) continue;
    const forced = run("/usr/bin/hdiutil", ["detach", "-force", device]);
    if (forced.status !== 0) {
      console.warn(`Could not unmount stale build disk image ${device}; continuing.`);
    }
  }
}

function findAppleDevelopmentIdentity() {
  const result = run("/usr/bin/security", ["find-identity", "-v", "-p", "codesigning"]);
  if (result.status !== 0) return null;

  const identities = [...(result.stdout ?? "").matchAll(/^\s*\d+\)\s+[0-9A-Fa-f]+\s+"([^"]+)"/gmu)]
    .map((match) => match[1]);
  return identities.find((identity) =>
    identity.startsWith("Apple Development:") || identity.startsWith("Mac Developer:"),
  ) ?? null;
}

function quitRunningCaptures() {
  const processNames = [APP_NAME, BINARY_NAME];
  if (!processNames.some(processIsRunning)) return;

  log("Quitting any running Captures instance…");
  run("/usr/bin/osascript", ["-e", `tell application "${APP_NAME}" to quit`]);
  spawnSync("/bin/sleep", ["0.4"], { stdio: "ignore" });
  for (const name of processNames) {
    if (processIsRunning(name)) run("/usr/bin/killall", [name]);
  }
  spawnSync("/bin/sleep", ["0.2"], { stdio: "ignore" });
  for (const name of processNames) {
    if (processIsRunning(name)) run("/usr/bin/killall", ["-9", name]);
  }
  spawnSync("/bin/sleep", ["0.2"], { stdio: "ignore" });
  const stillRunning = processNames.filter(processIsRunning);
  if (stillRunning.length > 0) {
    throw new Error(`Could not stop running Captures process: ${stillRunning.join(", ")}`);
  }
}

function stopRunningWindowsBuild() {
  const powershell = environment.SystemRoot
    ? join(environment.SystemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe")
    : "powershell.exe";
  const script = `
$target = [System.IO.Path]::GetFullPath($env:CAPTURES_BUILD_TARGET_EXE)
$matching = @(Get-Process -Name captures -ErrorAction SilentlyContinue | Where-Object {
  $_.Path -and [System.IO.Path]::GetFullPath($_.Path) -eq $target
})
foreach ($process in $matching) {
  Stop-Process -Id $process.Id -Force -ErrorAction Stop
  Wait-Process -Id $process.Id -Timeout 5 -ErrorAction SilentlyContinue
}
[Console]::Out.Write($matching.Count)
`;
  const result = runChecked(
    powershell,
    ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
    {
      env: {
        ...environment,
        CAPTURES_BUILD_TARGET_EXE: BUILT_WINDOWS_EXE,
      },
    },
  );
  const stopped = Number.parseInt(result.stdout.trim(), 10);
  if (!Number.isInteger(stopped) || stopped < 0) {
    throw new Error("PowerShell returned an invalid process count.");
  }
  if (stopped > 0) {
    log(`Stopped ${stopped} running checkout cop${stopped === 1 ? "y" : "ies"} of Captures.`);
  }
}

function resetScreenRecordingPermission() {
  const bundleId = "io.github.joswayski.captures";
  log(`Resetting Screen Recording permission for ${bundleId}…`);
  const result = run("/usr/bin/tccutil", ["reset", "ScreenCapture", bundleId], {
    stdio: "inherit",
  });
  if (result.status !== 0) {
    console.warn(
      "tccutil could not reset ScreenCapture (OK if nothing was granted yet). " +
        "Approve Captures in System Settings after launch.",
    );
  }
}

function installToApplications() {
  if (!existsSync(BUILT_APP)) {
    throw new Error(`Built app not found at ${BUILT_APP}`);
  }

  log(`Installing ${APP_BUNDLE} → ${APPLICATIONS_APP}…`);
  runChecked("/bin/rm", ["-rf", APPLICATIONS_APP]);
  runChecked("/usr/bin/ditto", [BUILT_APP, APPLICATIONS_APP]);
  run("/usr/bin/xattr", ["-dr", "com.apple.quarantine", APPLICATIONS_APP]);
  log(`Installed → ${APPLICATIONS_APP}`);
}

configureMacBuildEnvironment();

if (isMac && !environment.APPLE_SIGNING_IDENTITY) {
  const identity = findAppleDevelopmentIdentity();
  if (identity) {
    environment.APPLE_SIGNING_IDENTITY = identity;
    log(`Using the stable macOS development signing identity “${identity}”.`);
  } else {
    environment.APPLE_SIGNING_IDENTITY = "-";
    console.warn(
      "No Apple Development signing identity was found. Using an ad-hoc signature; macOS will require Screen Recording approval again whenever the executable changes. Create a development certificate in Xcode or set APPLE_SIGNING_IDENTITY to stop the repeated prompts.",
    );
  }
}

if (isMac) detachMountedBuildImages();
if (isWindows) {
  try {
    stopRunningWindowsBuild();
  } catch (error) {
    console.error(`Could not stop the running Windows build: ${error.message}`);
    process.exit(1);
  }
}
if (isMac) {
  log("Preparing the pinned LGPL FFmpeg sidecars…");
  runChecked("/bin/bash", ["scripts/build-ffmpeg-sidecars.sh"], { cwd: ROOT, stdio: "inherit" });
}

const npmCli = npmCliPath();
if (!npmCli) {
  console.error(
    "Could not locate npm's CLI. Run this build through `npm run build`, or reinstall Node.js with npm included.",
  );
  process.exit(1);
}
const args = ["run", "tauri:build", "--workspace", "@captures/desktop"];
const tauriArgs = [];
const buildConfig = isMac
  ? environment.TAURI_SIGNING_PRIVATE_KEY
    ? "src-tauri/tauri.recording.conf.json"
    : "src-tauri/tauri.recording.local.conf.json"
  : !environment.TAURI_SIGNING_PRIVATE_KEY
     ? "src-tauri/tauri.local.conf.json"
     : null;
if (buildConfig) {
  tauriArgs.push("--config", buildConfig);
}
tauriArgs.push(...process.argv.slice(2));
if (tauriArgs.length > 0) {
  args.push("--", ...tauriArgs);
}
const result = spawnSync(
  process.execPath,
  [npmCli, ...args],
  {
    env: environment,
    stdio: "inherit",
    cwd: ROOT,
  },
);

if (result.error) {
  console.error(`Failed to start the desktop build through npm: ${result.error.message}`);
  process.exit(1);
}

if ((result.status ?? 1) !== 0) {
  process.exit(result.status ?? 1);
}

if (isMac && !skipInstall) {
  try {
    quitRunningCaptures();
    if (resetPermissions) {
      resetScreenRecordingPermission();
    } else {
      log("Keeping existing Screen Recording permission (set CAPTURES_RESET_PERMISSIONS=1 to wipe it).");
    }
    installToApplications();
    if (openAfterInstall) {
      log(`Launching ${APP_NAME}…`);
      runChecked("/usr/bin/open", [APPLICATIONS_APP], { stdio: "inherit" });
      log("If Screen Recording was never granted for this build, approve it when prompted, then retry the shortcut once.");
    } else {
      log(`Launch with: open -a ${APP_NAME}`);
    }
  } catch (error) {
    console.error(`Build succeeded, but install failed: ${error.message}`);
    process.exit(1);
  }
} else if (isMac && skipInstall) {
  if (resetPermissions) {
    console.warn("Ignoring CAPTURES_RESET_PERMISSIONS=1 because CAPTURES_SKIP_INSTALL=1.");
  }
  log("Skipping Applications install (CAPTURES_SKIP_INSTALL=1).");
  log(`Bundle is at: ${BUILT_APP}`);
}

process.exit(0);
