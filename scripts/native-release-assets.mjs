import { createHash } from "node:crypto";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export const NATIVE_ASSETS = [
  "Captures-macOS-Apple-Silicon.dmg",
  "Captures-Windows-x64-setup.exe",
  "Captures-Linux-x64.deb",
  "Captures-Linux-x64.tar.gz",
];

/** A native release must never carry Tauri updater metadata or partial builds. */
export function validateNativeAssets(directory) {
  const entries = readdirSync(directory, { withFileTypes: true });
  const expected = [...NATIVE_ASSETS].sort();
  const actual = entries.map((entry) => entry.name).sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected) || entries.some((entry) => !entry.isFile())) {
    throw new Error(`Native Preview requires exactly: ${expected.join(", ")}`);
  }
  return expected.map((name) => {
    const bytes = readFileSync(join(directory, name));
    if (bytes.length === 0) throw new Error(`Empty native asset: ${name}`);
    return `${createHash("sha256").update(bytes).digest("hex")}  ${name}`;
  }).join("\n") + "\n";
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const directory = process.argv[2];
  if (!directory) throw new Error("usage: native-release-assets.mjs <directory>");
  const checksums = validateNativeAssets(directory);
  writeFileSync(join(directory, "SHA256SUMS"), checksums);
  process.stdout.write(checksums);
}
