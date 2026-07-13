import { spawnSync } from "node:child_process";

const environment = { ...process.env };

if (process.platform === "darwin" && !environment.APPLE_SIGNING_IDENTITY) {
  environment.APPLE_SIGNING_IDENTITY = "-";
  console.log("Using a local ad-hoc identity to seal the macOS app bundle.");
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
