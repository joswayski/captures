import { spawnSync } from "node:child_process";

const environment = { ...process.env };

function findAppleDevelopmentIdentity() {
  const result = spawnSync(
    "/usr/bin/security",
    ["find-identity", "-v", "-p", "codesigning"],
    { encoding: "utf8" },
  );
  if (result.status !== 0) return null;

  const identities = [...result.stdout.matchAll(/^\s*\d+\)\s+[0-9A-Fa-f]+\s+"([^"]+)"/gmu)]
    .map((match) => match[1]);
  return identities.find((identity) =>
    identity.startsWith("Apple Development:") || identity.startsWith("Mac Developer:"),
  ) ?? null;
}

if (process.platform === "darwin" && !environment.APPLE_SIGNING_IDENTITY) {
  const identity = findAppleDevelopmentIdentity();
  if (identity) {
    environment.APPLE_SIGNING_IDENTITY = identity;
    console.log(`Using the stable macOS development signing identity “${identity}”.`);
  } else {
    environment.APPLE_SIGNING_IDENTITY = "-";
    console.warn(
      "No Apple Development signing identity was found. Using an ad-hoc signature; macOS will require Screen Recording approval again whenever the executable changes. Create a development certificate in Xcode or set APPLE_SIGNING_IDENTITY to stop the repeated prompts.",
    );
  }
}

const npm = process.platform === "win32" ? "npm.cmd" : "npm";
const result = spawnSync(
  npm,
  ["run", "tauri:build", "--workspace", "@ces/desktop"],
  {
    env: environment,
    stdio: "inherit",
  },
);

if (result.error) {
  console.error(`Failed to start the desktop build: ${result.error.message}`);
  process.exit(1);
}

process.exit(result.status ?? 1);
