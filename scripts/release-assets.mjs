import { createHash } from "node:crypto";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const PLATFORM_KEYS = ["darwin-aarch64", "windows-x86_64", "linux-x86_64"];

export function validateAndWriteChecksums(directory, appVersion) {
  const root = resolve(directory);
  const names = readdirSync(root).sort();
  const requiredAssets = [
    ["macOS DMG", (name) => name.endsWith(".dmg")],
    ["macOS updater archive", (name) => name.endsWith(".app.tar.gz")],
    ["macOS updater signature", (name) => name.endsWith(".app.tar.gz.sig")],
    ["Windows NSIS installer", (name) => name.endsWith("-setup.exe") || name.endsWith("_setup.exe")],
    ["Windows updater signature", (name) => name.endsWith(".exe.sig")],
    ["Linux AppImage", (name) => name.endsWith(".AppImage")],
    ["Linux updater signature", (name) => name.endsWith(".AppImage.sig")],
    ["Linux Debian package", (name) => name.endsWith(".deb")],
    ["updater manifest", (name) => name === "latest.json"],
  ];
  for (const [label, predicate] of requiredAssets) {
    if (!names.some(predicate)) throw new Error(`release is missing its ${label}`);
  }

  const latest = JSON.parse(readFileSync(join(root, "latest.json"), "utf8"));
  if (latest.version !== appVersion) {
    throw new Error(`latest.json version ${latest.version} does not match ${appVersion}`);
  }
  if (typeof latest.notes !== "string" || latest.notes.trim() === "") {
    throw new Error("latest.json is missing release notes");
  }
  for (const platform of PLATFORM_KEYS) {
    const entry = latest.platforms?.[platform];
    if (!entry?.url || !entry?.signature) {
      throw new Error(`latest.json is missing a complete ${platform} entry`);
    }
  }

  const checksumNames = names.filter((name) => name !== "SHA256SUMS");
  const checksums = checksumNames.map((name) => {
    const hash = createHash("sha256").update(readFileSync(join(root, name))).digest("hex");
    return `${hash}  ${name}`;
  });
  writeFileSync(join(root, "SHA256SUMS"), `${checksums.join("\n")}\n`);
  return { names, checksums };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [directory, appVersion] = process.argv.slice(2);
  if (!directory || !appVersion) {
    throw new Error("usage: node scripts/release-assets.mjs <directory> <app-version>");
  }
  const result = validateAndWriteChecksums(directory, appVersion);
  process.stdout.write(`Validated ${result.names.length} release assets.\n`);
}
