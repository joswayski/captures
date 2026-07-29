import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const [appVersion] = process.argv.slice(2);
if (!/^\d+\.\d+\.\d+$/u.test(appVersion ?? "")) {
  throw new Error("usage: node scripts/prepare-release-build.mjs <app-version>");
}

const required = ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"];
if (process.platform === "darwin") {
  required.push(
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
    "KEYCHAIN_PASSWORD",
    "APPLE_API_ISSUER",
    "APPLE_API_KEY",
    "APPLE_API_PRIVATE_KEY",
  );
}
const missing = required.filter((name) => !process.env[name]);
if (missing.length > 0) {
  throw new Error(`missing release environment secrets: ${missing.join(", ")}`);
}

const releaseConfiguration = {
  ...JSON.parse(readFileSync(resolve("apps/desktop/src-tauri/tauri.recording.conf.json"), "utf8")),
  version: appVersion,
};
writeFileSync(
  resolve("apps/desktop/src-tauri/tauri.release.conf.json"),
  `${JSON.stringify(releaseConfiguration, null, 2)}\n`,
);

if (process.platform === "darwin") {
  const runnerTemp = process.env.RUNNER_TEMP;
  const githubEnv = process.env.GITHUB_ENV;
  if (!runnerTemp || !githubEnv) throw new Error("RUNNER_TEMP and GITHUB_ENV are required on macOS CI");

  const certificatePath = join(runnerTemp, "captures-developer-id.p12");
  const apiKeyPath = join(runnerTemp, `AuthKey_${process.env.APPLE_API_KEY}.p8`);
  writeFileSync(certificatePath, Buffer.from(process.env.APPLE_CERTIFICATE, "base64"), { mode: 0o600 });
  writeFileSync(apiKeyPath, process.env.APPLE_API_PRIVATE_KEY, { mode: 0o600 });
  appendFileSync(githubEnv, `APPLE_CERTIFICATE_PATH=${certificatePath}\nAPPLE_API_KEY_PATH=${apiKeyPath}\n`);
}
