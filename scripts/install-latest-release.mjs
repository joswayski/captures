import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { pathToFileURL } from "node:url";

import { latestPreviewRelease } from "./preview-release.mjs";

const APP_NAME = "Captures";
const BINARY_NAME = "captures";
const REPOSITORY = "joswayski/captures";
const COMPLETION_ASSET = "SHA256SUMS";
const DEFAULT_WAIT_MS = 60 * 60 * 1_000;
const POLL_MS = 15_000;

function log(message) {
  console.log(message);
}

function commandError(command, result) {
  const detail = result.error?.message || result.stderr?.trim();
  return new Error(detail ? `${command} failed: ${detail}` : `${command} failed with status ${result.status}`);
}

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    encoding: "utf8",
    stdio: options.stdio ?? "pipe",
    env: options.env ?? process.env,
    cwd: options.cwd,
  });
}

function runChecked(command, args, options = {}) {
  const result = run(command, args, options);
  if (result.error || result.status !== 0) throw commandError(command, result);
  return result;
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function commandExists(command, args = ["--version"]) {
  const result = run(command, args);
  return !result.error && result.status === 0;
}

export function parseOptions(args) {
  const options = {
    dryRun: false,
    launch: true,
    waitMs: DEFAULT_WAIT_MS,
  };

  for (const argument of args) {
    if (argument === "--dry-run") {
      options.dryRun = true;
      options.waitMs = 0;
    } else if (argument === "--no-launch") {
      options.launch = false;
    } else if (argument === "--no-wait") {
      options.waitMs = 0;
    } else if (argument === "--help" || argument === "-h") {
      options.help = true;
    } else {
      throw new Error(`unknown option: ${argument}`);
    }
  }

  return options;
}

export function platformSpec(platform, architecture, preferDebian = false) {
  if (platform === "darwin" && architecture === "arm64") {
    return {
      description: "macOS Apple Silicon DMG",
      matches: (name) => name.endsWith(".dmg"),
      packageType: "dmg",
    };
  }
  if (platform === "win32" && architecture === "x64") {
    return {
      description: "Windows x64 NSIS installer",
      matches: (name) => /(?:-|_)x64-setup\.exe$/iu.test(name) || /(?:-|_)setup\.exe$/iu.test(name),
      packageType: "nsis",
    };
  }
  if (platform === "linux" && architecture === "x64" && preferDebian) {
    return {
      description: "Debian/Ubuntu x64 package",
      matches: (name) => name.endsWith("_amd64.deb") || name.endsWith(".deb"),
      packageType: "deb",
    };
  }
  if (platform === "linux" && architecture === "x64") {
    return {
      description: "Linux x64 AppImage",
      matches: (name) => name.endsWith("_amd64.AppImage") || name.endsWith(".AppImage"),
      packageType: "appimage",
    };
  }

  const supported = "macOS Apple Silicon, Windows x64, and Linux x64";
  throw new Error(`no official Captures installer is built for ${platform}/${architecture}; supported systems: ${supported}`);
}

export function releaseReadiness(release, spec) {
  const uploadedAssets = (release.assets ?? []).filter((asset) => asset.state === "uploaded");
  const installAsset = uploadedAssets.find((asset) => spec.matches(asset.name)) ?? null;
  const checksumAsset = uploadedAssets.find((asset) => asset.name === COMPLETION_ASSET) ?? null;
  const missing = [];
  if (!installAsset) missing.push(spec.description);
  if (!checksumAsset) missing.push("complete-release validation");
  return {
    ready: missing.length === 0,
    installAsset,
    checksumAsset,
    missing,
  };
}

export function expectedChecksum(manifest, assetName) {
  for (const line of manifest.split(/\r?\n/u)) {
    const match = line.match(/^([a-fA-F0-9]{64})  (.+)$/u);
    if (match?.[2] === assetName) return match[1].toLowerCase();
  }
  throw new Error(`${COMPLETION_ASSET} does not contain ${assetName}`);
}

export function verifyChecksum(assetPath, manifestPath) {
  const name = basename(assetPath);
  const expected = expectedChecksum(readFileSync(manifestPath, "utf8"), name);
  const actual = createHash("sha256").update(readFileSync(assetPath)).digest("hex");
  if (actual !== expected) {
    throw new Error(`checksum mismatch for ${name}: expected ${expected}, received ${actual}`);
  }
  return actual;
}

function githubJson(endpoint, paginate = false) {
  const args = paginate
    ? ["api", "--paginate", "--slurp", endpoint]
    : ["api", endpoint];
  const result = runChecked("gh", args);
  try {
    const parsed = JSON.parse(result.stdout);
    return paginate ? parsed.flat() : parsed;
  } catch {
    throw new Error(`GitHub returned invalid JSON for ${endpoint}`);
  }
}

function confirmGitHubAccess() {
  const version = run("gh", ["--version"]);
  if (version.error?.code === "ENOENT") {
    throw new Error("GitHub CLI is required. Install `gh`, then run `gh auth login`.");
  }
  if (version.status !== 0) throw commandError("gh", version);

  const auth = run("gh", ["auth", "status"]);
  if (auth.status !== 0) {
    throw new Error("GitHub CLI must be signed in to access releases. Run `gh auth login`.");
  }
}

function fetchLatestRelease() {
  const releases = githubJson(`repos/${REPOSITORY}/releases?per_page=100`, true);
  const release = latestPreviewRelease(releases);
  if (!release) throw new Error(`no published Preview releases were found in ${REPOSITORY}`);
  return release;
}

async function waitForCompleteRelease(release, spec, waitMs) {
  const startedAt = Date.now();
  let current = release;
  let lastStatus = "";

  while (true) {
    const readiness = releaseReadiness(current, spec);
    if (readiness.ready) return { release: current, ...readiness };

    const status = readiness.missing.join(" and ");
    if (status !== lastStatus) {
      log(`Waiting for ${current.name} to finish ${status}…`);
      lastStatus = status;
    }
    if (Date.now() - startedAt >= waitMs) {
      throw new Error(
        `${current.name} is still missing ${status}. Its workflow may still be running; rerun this command when it finishes.`,
      );
    }

    await delay(POLL_MS);
    try {
      current = githubJson(`repos/${REPOSITORY}/releases/${current.id}`);
    } catch (error) {
      throw new Error(`${current.name} disappeared while it was building. Its release workflow likely failed: ${error.message}`);
    }
  }
}

function downloadReleaseAssets(release, installAsset, directory) {
  log(`Downloading ${installAsset.name} from ${release.name}…`);
  runChecked(
    "gh",
    [
      "release",
      "download",
      release.tag_name,
      "--repo",
      REPOSITORY,
      "--pattern",
      installAsset.name,
      "--pattern",
      COMPLETION_ASSET,
      "--dir",
      directory,
    ],
    { stdio: "inherit" },
  );

  const assetPath = join(directory, installAsset.name);
  const manifestPath = join(directory, COMPLETION_ASSET);
  if (!existsSync(assetPath) || !existsSync(manifestPath)) {
    throw new Error("GitHub did not download the expected installer and checksum manifest");
  }

  const checksum = verifyChecksum(assetPath, manifestPath);
  log(`Verified SHA-256 ${checksum}.`);
  return assetPath;
}

function processIsRunning(name) {
  return run("/usr/bin/pgrep", ["-x", name]).status === 0;
}

function quitMacCaptures() {
  const processNames = [APP_NAME, BINARY_NAME];
  if (!processNames.some(processIsRunning)) return;

  log("Quitting Captures…");
  run("/usr/bin/osascript", ["-e", `tell application "${APP_NAME}" to quit`]);
  run("/bin/sleep", ["0.4"]);
  for (const name of processNames) {
    if (processIsRunning(name)) run("/usr/bin/killall", [name]);
  }
  run("/bin/sleep", ["0.2"]);
  for (const name of processNames) {
    if (processIsRunning(name)) run("/usr/bin/killall", ["-9", name]);
  }
  const remaining = processNames.filter(processIsRunning);
  if (remaining.length > 0) throw new Error(`could not stop Captures process: ${remaining.join(", ")}`);
}

function installMacDmg(assetPath, directory, launch) {
  const applicationsApp = join("/Applications", `${APP_NAME}.app`);
  const mountPoint = join(directory, "mounted-dmg");
  mkdirSync(mountPoint);

  log("Validating the notarized macOS installer…");
  runChecked(
    "/usr/sbin/spctl",
    ["--assess", "--type", "open", "--context", "context:primary-signature", "--verbose=2", assetPath],
    { stdio: "inherit" },
  );

  let mounted = false;
  try {
    runChecked(
      "/usr/bin/hdiutil",
      ["attach", "-nobrowse", "-readonly", "-mountpoint", mountPoint, assetPath],
      { stdio: "inherit" },
    );
    mounted = true;

    const sourceApp = join(mountPoint, `${APP_NAME}.app`);
    if (!existsSync(sourceApp)) throw new Error(`the DMG does not contain ${APP_NAME}.app`);
    runChecked("/usr/bin/codesign", ["--verify", "--deep", "--strict", "--verbose=2", sourceApp], {
      stdio: "inherit",
    });

    quitMacCaptures();
    log(`Uninstalling ${applicationsApp}…`);
    runChecked("/bin/rm", ["-rf", applicationsApp]);
    log(`Installing ${APP_NAME}.app in /Applications…`);
    runChecked("/usr/bin/ditto", [sourceApp, applicationsApp]);
    if (!existsSync(applicationsApp)) {
      throw new Error(`${APP_NAME}.app was not installed in /Applications`);
    }
    runChecked("/usr/bin/codesign", ["--verify", "--deep", "--strict", "--verbose=2", applicationsApp], {
      stdio: "inherit",
    });
    runChecked("/usr/sbin/spctl", ["--assess", "--type", "execute", "--verbose=2", applicationsApp], {
      stdio: "inherit",
    });
  } finally {
    if (mounted) {
      const detached = run("/usr/bin/hdiutil", ["detach", mountPoint], { stdio: "inherit" });
      if (detached.status !== 0) {
        run("/usr/bin/hdiutil", ["detach", "-force", mountPoint], { stdio: "inherit" });
      }
    }
  }

  if (launch) runChecked("/usr/bin/open", [applicationsApp]);
  log(`Installed ${APP_NAME} from ${basename(assetPath)}.`);
}

function powershellPath() {
  return process.env.SystemRoot
    ? join(process.env.SystemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe")
    : "powershell.exe";
}

function installWindowsNsis(assetPath, launch) {
  const script = String.raw`
$ErrorActionPreference = "Stop"

$running = @(Get-Process -Name captures -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
  Write-Host "Quitting Captures..."
  foreach ($process in $running) {
    if ($process.MainWindowHandle -ne 0) {
      $null = $process.CloseMainWindow()
    }
  }
  Start-Sleep -Milliseconds 700
  Get-Process -Name captures -ErrorAction SilentlyContinue |
    Stop-Process -Force -ErrorAction Stop
}

$registryPaths = @(
  "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*",
  "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*",
  "HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*"
)

function Find-CapturesExecutable {
  $candidates = @(
    (Join-Path $env:LOCALAPPDATA "Captures\captures.exe"),
    (Join-Path $env:ProgramFiles "Captures\captures.exe")
  )
  $entries = @(
    Get-ItemProperty -Path $registryPaths -ErrorAction SilentlyContinue |
      Where-Object { $_.DisplayName -eq "Captures" }
  )
  foreach ($entry in $entries) {
    $installLocation = ([string]$entry.InstallLocation).Trim().Trim('"')
    if (-not [string]::IsNullOrWhiteSpace($installLocation)) {
      $candidates += Join-Path $installLocation "captures.exe"
    }

    $uninstallCommand = [string]$entry.UninstallString
    if (-not [string]::IsNullOrWhiteSpace($uninstallCommand)) {
      $uninstaller = if ($uninstallCommand -match '^"([^"]+)"') {
        $Matches[1]
      } else {
        ($uninstallCommand -split '\s+', 2)[0]
      }
      $candidates += Join-Path (Split-Path -Parent $uninstaller) "captures.exe"
    }
  }

  return $candidates |
    Where-Object { Test-Path -LiteralPath $_ } |
    Select-Object -Unique |
    Select-Object -First 1
}

$installed = @(
  Get-ItemProperty -Path $registryPaths -ErrorAction SilentlyContinue |
    Where-Object { $_.DisplayName -eq "Captures" }
)
foreach ($entry in $installed) {
  $command = [string]$entry.UninstallString
  if ([string]::IsNullOrWhiteSpace($command)) {
    continue
  }
  $uninstaller = if ($command -match '^"([^"]+)"') {
    $Matches[1]
  } else {
    ($command -split '\s+', 2)[0]
  }
  if (Test-Path -LiteralPath $uninstaller) {
    Write-Host "Uninstalling the existing Captures app..."
    $process = Start-Process -FilePath $uninstaller -ArgumentList "/S" -Wait -PassThru
    if ($process.ExitCode -ne 0) {
      throw "Captures uninstaller exited with code $($process.ExitCode)."
    }
  }
}

Write-Host "Installing the latest Captures app..."
$installer = Start-Process -FilePath $env:CAPTURES_INSTALLER -ArgumentList "/S" -Wait -PassThru
if ($installer.ExitCode -ne 0) {
  throw "Captures installer exited with code $($installer.ExitCode)."
}

$application = Find-CapturesExecutable
if (-not $application) {
  throw "Captures installer exited successfully, but captures.exe was not found."
}
Write-Host "Verified installed executable at $application."

if ($env:CAPTURES_LAUNCH_AFTER_INSTALL -eq "1") {
  Start-Process -FilePath $application
}
`;

  runChecked(
    powershellPath(),
    ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
    {
      stdio: "inherit",
      env: {
        ...process.env,
        CAPTURES_INSTALLER: assetPath,
        CAPTURES_LAUNCH_AFTER_INSTALL: launch ? "1" : "0",
      },
    },
  );
  log(`Installed ${APP_NAME} from ${basename(assetPath)}.`);
}

function quitLinuxCaptures() {
  const result = run("pkill", ["-x", BINARY_NAME]);
  if (result.error?.code === "ENOENT") return;
  if (result.status !== 0 && result.status !== 1) throw commandError("pkill", result);
}

function launchDetached(command) {
  const child = spawn(command, [], {
    detached: true,
    stdio: "ignore",
  });
  child.once("error", (error) => {
    console.warn(`${APP_NAME} installed, but it could not be launched automatically: ${error.message}`);
  });
  child.unref();
}

function installDebianPackage(assetPath, launch) {
  const packageName = runChecked("dpkg-deb", ["--field", assetPath, "Package"]).stdout.trim();
  if (!/^[a-z0-9][a-z0-9+.-]+$/u.test(packageName)) {
    throw new Error(`the Debian package has an invalid package name: ${packageName}`);
  }

  log("Refreshing Debian package metadata…");
  runChecked("sudo", ["apt-get", "update"], { stdio: "inherit" });
  quitLinuxCaptures();
  const installed = run("dpkg-query", ["--show", "--showformat=${db:Status-Abbrev}", packageName]);
  if (installed.status === 0 && installed.stdout.startsWith("ii")) {
    log(`Uninstalling the existing ${packageName} package…`);
    runChecked("sudo", ["apt-get", "remove", "--yes", packageName], { stdio: "inherit" });
  }
  log(`Installing ${basename(assetPath)}…`);
  runChecked("sudo", ["apt-get", "install", "--yes", assetPath], { stdio: "inherit" });
  const verified = runChecked("dpkg-query", ["--show", "--showformat=${db:Status-Abbrev}", packageName]);
  if (!verified.stdout.startsWith("ii")) {
    throw new Error(`${packageName} was not registered as an installed Debian package`);
  }
  if (launch) launchDetached(BINARY_NAME);
  log(`Installed ${APP_NAME} from ${basename(assetPath)}.`);
}

function installAppImage(assetPath, launch) {
  const destination = join(homedir(), ".local", "bin", "Captures.AppImage");
  mkdirSync(dirname(destination), { recursive: true });
  quitLinuxCaptures();
  log(`Uninstalling ${destination}…`);
  rmSync(destination, { force: true });
  log(`Installing ${basename(assetPath)} → ${destination}…`);
  copyFileSync(assetPath, destination);
  chmodSync(destination, 0o755);
  if (!existsSync(destination) || (statSync(destination).mode & 0o111) === 0) {
    throw new Error(`${APP_NAME} AppImage was not installed as an executable at ${destination}`);
  }
  if (launch) launchDetached(destination);
  log(`Installed ${APP_NAME} at ${destination}.`);
}

function printHelp() {
  console.log(`Usage: npm run install:preview -- [options]

Downloads, verifies, and installs the newest complete Preview for this system.
The command verifies the release checksum before changing the installed app.

Options:
  --dry-run    Report the newest Preview's status without downloading or installing
  --no-launch  Do not open Captures after installation
  --no-wait    Fail immediately if the newest Preview is incomplete
  --help       Show this help`);
}

export async function main(args = process.argv.slice(2)) {
  const options = parseOptions(args);
  if (options.help) {
    printHelp();
    return;
  }

  confirmGitHubAccess();
  const preferDebian = process.platform === "linux"
    && commandExists("dpkg-deb")
    && commandExists("apt-get", ["--version"]);
  const spec = platformSpec(process.platform, process.arch, preferDebian);
  const release = fetchLatestRelease();
  log(`Newest Preview: ${release.name} (${release.tag_name}).`);

  const initialReadiness = releaseReadiness(release, spec);
  if (options.dryRun) {
    if (initialReadiness.ready) {
      log(`Ready to install ${initialReadiness.installAsset.name}.`);
    } else {
      log(`Still building: ${initialReadiness.missing.join(" and ")}.`);
    }
    return;
  }

  const complete = await waitForCompleteRelease(release, spec, options.waitMs);
  const directory = mkdtempSync(join(tmpdir(), "captures-release-install-"));
  try {
    const assetPath = downloadReleaseAssets(complete.release, complete.installAsset, directory);
    if (spec.packageType === "dmg") {
      installMacDmg(assetPath, directory, options.launch);
    } else if (spec.packageType === "nsis") {
      installWindowsNsis(assetPath, options.launch);
    } else if (spec.packageType === "deb") {
      installDebianPackage(assetPath, options.launch);
    } else {
      installAppImage(assetPath, options.launch);
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(`Could not install the latest Captures Preview: ${error.message}`);
    process.exitCode = 1;
  });
}
