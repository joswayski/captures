import assert from "node:assert/strict";
import test from "node:test";

import {
  applyOnboardingReset,
  DEFAULT_NOTARY_PROFILE,
  downloadQuarantineAttribute,
  selectNewestDmg,
  findDeveloperIdIdentity,
  main,
  mergeNotarizationEnv,
  missingCredentialMessage,
  notarizationEnvContents,
  notarizationEnvPath,
  parseEnvFile,
  parseOptions,
  parseSigningIdentities,
  resolveNotarizationCredentials,
} from "./build-signed-local.mjs";

test("parses local signed-build flags", () => {
  assert.deepEqual(parseOptions([]), {
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
  });

  const setup = parseOptions([
    "--setup",
    "--key",
    "/tmp/AuthKey_ABC.p8",
    "--key-id",
    "ABC",
    "--issuer",
    "issuer-id",
    "--dry-run",
  ]);
  assert.equal(setup.setup, true);
  assert.equal(setup.keyPath, "/tmp/AuthKey_ABC.p8");
  assert.equal(setup.keyId, "ABC");
  assert.equal(setup.issuer, "issuer-id");
  assert.equal(setup.dryRun, true);

  const skip = parseOptions(["--skip-notarize", "--no-launch", "--keep-onboarding", "--keep-permissions"]);
  assert.equal(skip.notarize, false);
  assert.equal(skip.quarantine, false);
  assert.equal(skip.launch, false);
  assert.equal(skip.resetOnboarding, false);
  assert.equal(skip.resetPermissions, false);
});

test("rejects unknown or incomplete options", () => {
  assert.throws(() => parseOptions(["--nope"]), /unknown option/u);
  assert.throws(() => parseOptions(["--key"]), /--key requires a value/u);
  assert.throws(() => parseOptions(["--key", "--issuer"]), /--key requires a value/u);
});

test("selects a Developer ID Application identity and ignores development certs", () => {
  const identities = parseSigningIdentities(`
  1) AABBCC "Apple Development: Jose Valerio (TEAMID)"
  2) DDEEFF "Developer ID Application: JOSE VALERIO (NAUU43LATL)"
  3) 112233 "Developer ID Installer: JOSE VALERIO (NAUU43LATL)"
`);
  assert.deepEqual(identities, [
    "Apple Development: Jose Valerio (TEAMID)",
    "Developer ID Application: JOSE VALERIO (NAUU43LATL)",
    "Developer ID Installer: JOSE VALERIO (NAUU43LATL)",
  ]);
  assert.equal(
    findDeveloperIdIdentity(identities),
    "Developer ID Application: JOSE VALERIO (NAUU43LATL)",
  );
  assert.equal(findDeveloperIdIdentity(["Apple Development: Jose Valerio (TEAMID)"]), null);
});

test("loads notarization env files without overriding the process environment", () => {
  const parsed = parseEnvFile(`
# comment
APPLE_API_ISSUER=from-file
APPLE_API_KEY="KEYID"
APPLE_API_KEY_PATH='/tmp/key.p8'

ignored
`);
  assert.deepEqual(parsed, {
    APPLE_API_ISSUER: "from-file",
    APPLE_API_KEY: "KEYID",
    APPLE_API_KEY_PATH: "/tmp/key.p8",
  });

  const merged = mergeNotarizationEnv(
    { APPLE_API_ISSUER: "from-env" },
    parsed,
  );
  assert.equal(merged.APPLE_API_ISSUER, "from-env");
  assert.equal(merged.APPLE_API_KEY, "KEYID");
  assert.equal(merged.APPLE_API_KEY_PATH, "/tmp/key.p8");
  assert.equal(notarizationEnvPath("/Users/jose"), "/Users/jose/.captures/notarization.env");
});

test("resolves API-key credentials or a stored notarytool profile", () => {
  assert.equal(resolveNotarizationCredentials({ env: {} }), null);
  assert.deepEqual(
    resolveNotarizationCredentials({
      env: {
        APPLE_API_ISSUER: "issuer",
        APPLE_API_KEY: "key",
        APPLE_API_KEY_PATH: "/tmp/key.p8",
      },
    }),
    {
      issuer: "issuer",
      keyId: "key",
      keyPath: "/tmp/key.p8",
      profile: null,
    },
  );
  assert.deepEqual(
    resolveNotarizationCredentials({ env: {}, profileExists: true }),
    {
      issuer: null,
      keyId: null,
      keyPath: null,
      profile: DEFAULT_NOTARY_PROFILE,
    },
  );
});

test("writes a private env file and explains the one-time setup", () => {
  assert.equal(
    notarizationEnvContents({
      issuer: "issuer",
      keyId: "KEYID",
      keyPath: "/Users/jose/.captures/AuthKey_KEYID.p8",
    }),
    [
      "# Local App Store Connect API key for notarizing Captures.",
      "# Keep this file and the .p8 private. Do not commit either.",
      "APPLE_API_ISSUER=issuer",
      "APPLE_API_KEY=KEYID",
      "APPLE_API_KEY_PATH=/Users/jose/.captures/AuthKey_KEYID.p8",
      "",
    ].join("\n"),
  );
  assert.match(missingCredentialMessage(), /npm run build:signed -- --setup/u);
  assert.match(missingCredentialMessage(), /~\/\.captures/u);
});

test("resets first-run onboarding without dropping other settings", () => {
  const next = applyOnboardingReset(
    JSON.stringify({
      onboarding_completed: true,
      launch_at_login: true,
    }),
  );
  assert.deepEqual(JSON.parse(next), {
    onboarding_completed: false,
    launch_at_login: true,
  });
});

test("picks the built DMG and formats a Safari-style quarantine bit", () => {
  assert.equal(
    selectNewestDmg([
      { name: "icon.icns", mtimeMs: 3 },
      { name: "Captures_0.1.0_aarch64.dmg", mtimeMs: 1 },
      { name: "Captures_0.1.1_aarch64.dmg", mtimeMs: 2 },
      { name: ".hidden.dmg", mtimeMs: 4 },
    ]),
    "Captures_0.1.1_aarch64.dmg",
  );
  assert.equal(selectNewestDmg([{ name: "icon.icns", mtimeMs: 1 }]), null);
  assert.equal(downloadQuarantineAttribute(0x68f1c000), "0081;68f1c000;Safari;");
});

test("dry-run reports missing notarization credentials without building", () => {
  const lines = [];
  const originalLog = console.log;
  console.log = (message) => {
    lines.push(String(message));
  };
  try {
    main(["--dry-run"], {
      platform: "darwin",
      home: "/tmp/captures-no-notarization-home",
      notaryProfileExists: false,
      env: {
        APPLE_SIGNING_IDENTITY: "Developer ID Application: JOSE VALERIO (NAUU43LATL)",
      },
    });
  } finally {
    console.log = originalLog;
  }
  assert.match(lines.join("\n"), /Using Developer ID Application: JOSE VALERIO \(NAUU43LATL\)/u);
  assert.match(lines.join("\n"), /--setup --key/u);
});

test("refuses to run the signed installer path off macOS", () => {
  assert.throws(
    () => main([], { platform: "linux" }),
    /macOS-only/u,
  );
});
