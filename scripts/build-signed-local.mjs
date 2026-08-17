import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const APP_NAME = "Captures";
const BINARY_NAME = "captures";
const BUNDLE_ID = "io.github.joswayski.captures";
const APP_BUNDLE = `${APP_NAME}.app`;
const APPLICATIONS_APP = join("/Applications", APP_BUNDLE);
const BUILT_APP = join(ROOT, "target/release/bundle/macos", APP_BUNDLE);
const DMG_DIRECTORY = join(ROOT, "target/release/bundle/dmg");
export const DEFAULT_NOTARY_PROFILE = "captures-notary";
export const NOTARIZATION_DIRECTORY_NAME = ".captures";
export const NOTARIZATION_ENV_FILE_NAME = "notarization.env";

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

function optionValue(args, index, flag) {
  const value = args[index];
  if (!value || value.startsWith("-")) {
    throw new Error(`${flag} requires a value`);
  }
  return value;
}

export function parseOptions(args) {
  const options = {
    dryRun: false,
    launch: true,
    help: false,
    setup: false,
    resetOnboarding: true,
    resetPermissions: true,
    freshSettings: false,
    notarize: true,
    quarantine: true,
    keyPath: null,
    keyId: null,
    issuer: null,
  };

  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--dry-run") {
      options.dryRun = true;
    } else if (argument === "--no-launch") {
      options.launch = false;
    } else if (argument === "--help" || argument === "-h") {
      options.help = true;
    } else if (argument === "--setup") {
      options.setup = true;
    } else if (argument === "--keep-onboarding") {
      options.resetOnboarding = false;
    } else if (argument === "--keep-permissions") {
      options.resetPermissions = false;
    } else if (argument === "--fresh-settings") {
      options.freshSettings = true;
    } else if (argument === "--skip-notarize") {
      options.notarize = false;
    } else if (argument === "--no-quarantine") {
      options.quarantine = false;
    } else if (argument === "--key") {
      options.keyPath = optionValue(args, (index += 1), argument);
    } else if (argument === "--key-id") {
      options.keyId = optionValue(args, (index += 1), argument);
    } else if (argument === "--issuer") {
      options.issuer = optionValue(args, (index += 1), argument);
    } else {
      throw new Error(`unknown option: ${argument}`);
    }
  }

  if (!options.notarize) {
    options.quarantine = false;
  }

  return options;
}

export function printHelp() {
  console.log(`Build a Developer ID-signed, notarized macOS DMG and install it the way a
downloaded Preview arrives: from the DMG, with Gatekeeper quarantine.

  npm run build:signed
  npm run build:signed -- --setup --key ~/AuthKey_XXXXXXXXXX.p8 --key-id XXXXXXXXXX --issuer <issuer-id>

The Developer ID Application identity name is not a secret. It is printed by
codesign on every shipped app. The .p12 private key, its password, and the
App Store Connect .p8 are secrets. Keep those out of the repo.

  --setup              Store the App Store Connect API key for later builds
  --key PATH           Path to the .p8 (with --setup)
  --key-id ID          App Store Connect key ID (with --setup)
  --issuer ID          App Store Connect issuer ID (with --setup)
  --dry-run            Resolve identity and credentials without building
  --no-launch          Install without opening Captures
  --keep-onboarding    Leave the current first-run flag alone
  --keep-permissions   Leave Screen Recording and Microphone grants alone
  --fresh-settings     Delete settings.json for a brand-new first launch
  --skip-notarize      Sign with Developer ID but skip Apple notarization
  --no-quarantine      Install without the downloaded-from-the-internet bit
  --help               Show this help`);
}

export function parseSigningIdentities(securityOutput) {
  return [...(securityOutput ?? "").matchAll(/^\s*\d+\)\s+[0-9A-Fa-f]+\s+"([^"]+)"/gmu)].map(
    (match) => match[1],
  );
}

export function findDeveloperIdIdentity(identities) {
  return identities.find((identity) => identity.startsWith("Developer ID Application:")) ?? null;
}

export function parseEnvFile(contents) {
  const values = {};
  for (const line of contents.split(/\r?\n/u)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const separator = trimmed.indexOf("=");
    if (separator === -1) continue;
    const key = trimmed.slice(0, separator).trim();
    let value = trimmed.slice(separator + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }
    values[key] = value;
  }
  return values;
}

export function notarizationDirectory(home = homedir()) {
  return join(home, NOTARIZATION_DIRECTORY_NAME);
}

export function notarizationEnvPath(home = homedir()) {
  return join(notarizationDirectory(home), NOTARIZATION_ENV_FILE_NAME);
}

export function notarizationEnvContents({ issuer, keyId, keyPath }) {
  return [
    "# Local App Store Connect API key for notarizing Captures.",
    "# Keep this file and the .p8 private. Do not commit either.",
    `APPLE_API_ISSUER=${issuer}`,
    `APPLE_API_KEY=${keyId}`,
    `APPLE_API_KEY_PATH=${keyPath}`,
    "",
  ].join("\n");
}

export function mergeNotarizationEnv(env, fileValues) {
  const merged = { ...env };
  for (const key of ["APPLE_API_ISSUER", "APPLE_API_KEY", "APPLE_API_KEY_PATH", "APPLE_NOTARY_PROFILE"]) {
    if (!merged[key] && fileValues[key]) merged[key] = fileValues[key];
  }
  return merged;
}

export function resolveNotarizationCredentials({ env = {}, profileExists = false } = {}) {
  const issuer = env.APPLE_API_ISSUER;
  const keyId = env.APPLE_API_KEY;
  const keyPath = env.APPLE_API_KEY_PATH;
  const hasApiKey = Boolean(issuer && keyId && keyPath);
  const profile = env.APPLE_NOTARY_PROFILE || (profileExists ? DEFAULT_NOTARY_PROFILE : null);
  if (!hasApiKey && !profile) return null;
  return {
    issuer: issuer ?? null,
    keyId: keyId ?? null,
    keyPath: keyPath ?? null,
    profile,
  };
}

export function missingCredentialMessage() {
  return `App Store Connect notarization credentials are not configured.

The Developer ID certificate in your keychain can sign Captures, but Gatekeeper
still needs a notarized, stapled DMG. GitHub will not give the release
environment secrets back. Use the .p8 you backed up when creating the App Store
Connect API key:

  npm run build:signed -- --setup --key ~/AuthKey_XXXXXXXXXX.p8 --key-id XXXXXXXXXX --issuer <issuer-id>

That stores the key in ~/.captures (mode 0600) and a notarytool keychain
profile named ${DEFAULT_NOTARY_PROFILE}. Later builds only need:

  npm run build:signed`;
}

export function onboardingSettingsPath(home = homedir()) {
  return join(home, "Library", "Application Support", "io.github.captures", "settings.json");
}

export function applyOnboardingReset(contents) {
  const settings = JSON.parse(contents);
  settings.onboarding_completed = false;
  return `${JSON.stringify(settings, null, 2)}\n`;
}

export function downloadQuarantineAttribute(timestampSeconds) {
  return `0081;${Number(timestampSeconds).toString(16)};Safari;`;
}

export function selectNewestDmg(entries) {
  let selected = null;
  for (const entry of entries) {
    if (!entry.name.endsWith(".dmg") || entry.name.startsWith(".")) continue;
    if (!selected || entry.mtimeMs >= selected.mtimeMs) selected = entry;
  }
  return selected?.name ?? null;
}

function notaryProfileExists(profile) {
  const result = run("/usr/bin/security", ["find-generic-password", "-s", "com.apple.gke.notary.tool", "-a", profile]);
  return result.status === 0;
}

function loadFileValues(home) {
  const path = notarizationEnvPath(home);
  if (!existsSync(path)) return {};
  return parseEnvFile(readFileSync(path, "utf8"));
}

function resolveIdentity(env) {
  if (env.APPLE_SIGNING_IDENTITY) return env.APPLE_SIGNING_IDENTITY;
  const result = run("/usr/bin/security", ["find-identity", "-v", "-p", "codesigning"]);
  if (result.status !== 0) {
    throw new Error("Could not list code-signing identities from the keychain.");
  }
  const identity = findDeveloperIdIdentity(parseSigningIdentities(result.stdout));
  if (!identity) {
    throw new Error(
      "No Developer ID Application identity was found in the keychain. Import the same .p12 used by the GitHub release environment, then retry.",
    );
  }
  return identity;
}

function resolveCredentials(env, home, options, runtime = {}) {
  const merged = mergeNotarizationEnv(env, loadFileValues(home));
  if (options.issuer) merged.APPLE_API_ISSUER = options.issuer;
  if (options.keyId) merged.APPLE_API_KEY = options.keyId;
  if (options.keyPath) merged.APPLE_API_KEY_PATH = options.keyPath;
  const profile = merged.APPLE_NOTARY_PROFILE || DEFAULT_NOTARY_PROFILE;
  const profileExists = runtime.notaryProfileExists ?? notaryProfileExists(profile);
  return {
    env: merged,
    credentials: resolveNotarizationCredentials({
      env: merged,
      profileExists,
    }),
  };
}

function setupNotarization(options, env, home) {
  const issuer = options.issuer || env.APPLE_API_ISSUER;
  const keyId = options.keyId || env.APPLE_API_KEY;
  const sourceKey = options.keyPath || env.APPLE_API_KEY_PATH;
  if (!issuer || !keyId || !sourceKey) {
    throw new Error(
      "--setup requires --key, --key-id, and --issuer (or APPLE_API_KEY_PATH, APPLE_API_KEY, and APPLE_API_ISSUER).",
    );
  }
  if (!existsSync(sourceKey)) {
    throw new Error(`App Store Connect API key not found at ${sourceKey}`);
  }

  const directory = notarizationDirectory(home);
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  const keyPath = join(directory, `AuthKey_${keyId}.p8`);
  copyFileSync(sourceKey, keyPath);
  chmodSync(keyPath, 0o600);

  const envPath = notarizationEnvPath(home);
  writeFileSync(envPath, notarizationEnvContents({ issuer, keyId, keyPath }));
  chmodSync(envPath, 0o600);
  chmodSync(directory, 0o700);

  runChecked(
    "/usr/bin/xcrun",
    [
      "notarytool",
      "store-credentials",
      DEFAULT_NOTARY_PROFILE,
      "--key",
      keyPath,
      "--key-id",
      keyId,
      "--issuer",
      issuer,
    ],
    { stdio: "inherit" },
  );
  log(`Stored notarization credentials in ${envPath} and the ${DEFAULT_NOTARY_PROFILE} notarytool profile.`);
  log("You can now run: npm run build:signed");
}

function processIsRunning(name) {
  return run("/usr/bin/pgrep", ["-x", name]).status === 0;
}

function quitRunningCaptures() {
  const processNames = [APP_NAME, BINARY_NAME];
  if (!processNames.some(processIsRunning)) return;

  log("Quitting any running Captures instance…");
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
  if (remaining.length > 0) {
    throw new Error(`Could not stop running Captures process: ${remaining.join(", ")}`);
  }
}

function resetOnboarding(options, home) {
  const path = onboardingSettingsPath(home);
  if (options.freshSettings) {
    if (existsSync(path)) {
      rmSync(path);
      log(`Deleted ${path} for a brand-new first launch.`);
    }
    return;
  }
  if (!options.resetOnboarding) return;
  if (!existsSync(path)) {
    log("No settings file yet; the next launch is already a first run.");
    return;
  }
  writeFileSync(path, applyOnboardingReset(readFileSync(path, "utf8")));
  log("Reset onboarding_completed so the next launch shows setup.");
}

function resetPermissions() {
  log(`Resetting Screen Recording and Microphone permission for ${BUNDLE_ID}…`);
  for (const service of ["ScreenCapture", "Microphone"]) {
    const result = run("/usr/bin/tccutil", ["reset", service, BUNDLE_ID], { stdio: "inherit" });
    if (result.status !== 0) {
      console.warn(`tccutil could not reset ${service} (OK if nothing was granted yet).`);
    }
  }
}

function buildSignedApp(env, identity) {
  const buildScript = join(ROOT, "scripts/build.mjs");
  const result = spawnSync(process.execPath, [buildScript], {
    cwd: ROOT,
    env: {
      ...env,
      APPLE_SIGNING_IDENTITY: identity,
      CAPTURES_SKIP_INSTALL: "1",
    },
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if ((result.status ?? 1) !== 0) {
    throw new Error("Signed desktop build failed.");
  }
  if (!existsSync(BUILT_APP)) {
    throw new Error(`Built app not found at ${BUILT_APP}`);
  }
}

function builtDmgPath() {
  if (!existsSync(DMG_DIRECTORY)) {
    throw new Error(`DMG output directory not found at ${DMG_DIRECTORY}`);
  }
  const name = selectNewestDmg(
    readdirSync(DMG_DIRECTORY).map((entryName) => ({
      name: entryName,
      mtimeMs: statSync(join(DMG_DIRECTORY, entryName)).mtimeMs,
    })),
  );
  if (!name) throw new Error(`No DMG was produced in ${DMG_DIRECTORY}`);
  return join(DMG_DIRECTORY, name);
}

function notarytoolArgs(credentials, dmg) {
  if (credentials.keyPath && credentials.keyId && credentials.issuer) {
    return [
      "notarytool",
      "submit",
      dmg,
      "--key",
      credentials.keyPath,
      "--key-id",
      credentials.keyId,
      "--issuer",
      credentials.issuer,
      "--wait",
    ];
  }
  return ["notarytool", "submit", dmg, "--keychain-profile", credentials.profile, "--wait"];
}

function notarizeAndStaple(dmg, app, credentials) {
  log(`Submitting ${basename(dmg)} to Apple notarization…`);
  runChecked("/usr/bin/xcrun", notarytoolArgs(credentials, dmg), { stdio: "inherit" });
  log("Stapling the notarization ticket…");
  runChecked("/usr/bin/xcrun", ["stapler", "staple", "-v", dmg], { stdio: "inherit" });
  runChecked("/usr/bin/xcrun", ["stapler", "staple", "-v", app], { stdio: "inherit" });
}

function validateSignedApp(app) {
  runChecked("/usr/bin/codesign", ["--verify", "--deep", "--strict", "--verbose=2", app], {
    stdio: "inherit",
  });
}

function validateNotarizedBundle(app, dmg) {
  validateSignedApp(app);
  runChecked("/usr/sbin/spctl", ["--assess", "--type", "execute", "--verbose=2", app], {
    stdio: "inherit",
  });
  runChecked("/usr/bin/xcrun", ["stapler", "validate", app], { stdio: "inherit" });
  runChecked("/usr/bin/codesign", ["--verify", "--strict", "--verbose=2", dmg], { stdio: "inherit" });
  runChecked(
    "/usr/sbin/spctl",
    ["--assess", "--type", "open", "--context", "context:primary-signature", "--verbose=2", dmg],
    { stdio: "inherit" },
  );
  runChecked("/usr/bin/xcrun", ["stapler", "validate", dmg], { stdio: "inherit" });
}

function installFromDmg(dmg, { quarantine }) {
  const mountPoint = join(ROOT, "target/release/bundle/dmg/.captures-signed-mount");
  rmSync(mountPoint, { recursive: true, force: true });
  mkdirSync(mountPoint, { recursive: true });

  let mounted = false;
  try {
    runChecked(
      "/usr/bin/hdiutil",
      ["attach", "-nobrowse", "-readonly", "-mountpoint", mountPoint, dmg],
      { stdio: "inherit" },
    );
    mounted = true;
    const sourceApp = join(mountPoint, APP_BUNDLE);
    if (!existsSync(sourceApp)) {
      throw new Error(`the DMG does not contain ${APP_BUNDLE}`);
    }

    quitRunningCaptures();
    log(`Installing ${APP_BUNDLE} from the notarized DMG…`);
    runChecked("/bin/rm", ["-rf", APPLICATIONS_APP]);
    runChecked("/usr/bin/ditto", [sourceApp, APPLICATIONS_APP]);
    if (!existsSync(APPLICATIONS_APP)) {
      throw new Error(`${APP_BUNDLE} was not installed in /Applications`);
    }
    runChecked("/usr/bin/xcrun", ["stapler", "staple", APPLICATIONS_APP], { stdio: "inherit" });
    validateSignedApp(APPLICATIONS_APP);
    runChecked("/usr/sbin/spctl", ["--assess", "--type", "execute", "--verbose=2", APPLICATIONS_APP], {
      stdio: "inherit",
    });

    if (quarantine) {
      const attribute = downloadQuarantineAttribute(Math.floor(Date.now() / 1000));
      runChecked("/usr/bin/xattr", ["-w", "com.apple.quarantine", attribute, APPLICATIONS_APP]);
      log("Applied a Safari-style quarantine bit so first launch hits Gatekeeper.");
    }
  } finally {
    if (mounted) {
      const detached = run("/usr/bin/hdiutil", ["detach", mountPoint], { stdio: "inherit" });
      if (detached.status !== 0) {
        run("/usr/bin/hdiutil", ["detach", "-force", mountPoint], { stdio: "inherit" });
      }
    }
    rmSync(mountPoint, { recursive: true, force: true });
  }
}

export function main(args = process.argv.slice(2), runtime = {}) {
  const options = parseOptions(args);
  if (options.help) {
    printHelp();
    return;
  }

  const platform = runtime.platform ?? process.platform;
  if (platform !== "darwin") {
    throw new Error("npm run build:signed is macOS-only. Use npm run build on Windows and Linux.");
  }

  const env = runtime.env ?? process.env;
  const home = runtime.home ?? homedir();

  if (options.setup) {
    if (options.dryRun) {
      log("Would store App Store Connect notarization credentials in ~/.captures.");
      return;
    }
    setupNotarization(options, env, home);
    return;
  }

  const identity = resolveIdentity(env);
  const { env: notarizationEnv, credentials } = resolveCredentials(env, home, options, runtime);
  log(`Using ${identity}.`);

  if (!options.notarize) {
    log("Skipping notarization; this will not reproduce Gatekeeper or the downloaded DMG path.");
  }

  if (options.dryRun) {
    if (options.notarize && !credentials) {
      log(missingCredentialMessage());
      return;
    }
    log(
      options.notarize
        ? "Ready to build, notarize, staple, and install from the DMG."
        : "Ready to build a Developer ID-signed app without notarization.",
    );
    return;
  }

  if (options.notarize && !credentials) {
    throw new Error(missingCredentialMessage());
  }

  resetOnboarding(options, home);
  if (options.resetPermissions) resetPermissions();

  buildSignedApp(notarizationEnv, identity);
  const dmg = builtDmgPath();
  validateSignedApp(BUILT_APP);

  if (options.notarize) {
    notarizeAndStaple(dmg, BUILT_APP, credentials);
    validateNotarizedBundle(BUILT_APP, dmg);
  }

  installFromDmg(dmg, { quarantine: options.quarantine });
  if (options.launch) {
    log(`Launching ${APP_NAME}…`);
    if (options.quarantine) {
      log("Gatekeeper should ask whether to open this downloaded app. That dialog is the point.");
    }
    runChecked("/usr/bin/open", [APPLICATIONS_APP], { stdio: "inherit" });
  } else {
    log(`Launch with: open ${APPLICATIONS_APP}`);
  }
  log(`Installed the local signed build from ${basename(dmg)}.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    console.error(`Could not build a signed local Captures preview: ${error.message}`);
    process.exitCode = 1;
  }
}
