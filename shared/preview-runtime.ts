/** Runtime identity written by the validated public Tauri release job (until November 2026). */
export const TAURI_RELEASE_ASSET = "captures-tauri.json";
/** Runtime identity written only by the validated public native release job. */
export const NATIVE_RELEASE_ASSET = "captures-native.json";

function hasUploadedAsset(release: { assets?: { name: string; state?: string }[] }, name: string) {
  return (
    release.assets?.some((asset) => asset.name === name && asset.state === "uploaded") ?? false
  );
}

/**
 * Public downloads and updates: validated native releases and earlier Tauri
 * releases. Unmarked October 2026 native archives stay excluded.
 */
export function isPublicPreview(release: {
  tag_name: string;
  assets?: { name: string; state?: string }[];
}): boolean {
  if (!/^v\d{4}\.\d{2}\.\d{2}\.[1-9]\d?$/u.test(release.tag_name)) return false;
  return hasUploadedAsset(release, NATIVE_RELEASE_ASSET) || isTauriPreview(release);
}

export function isTauriPreview(release: {
  tag_name: string;
  assets?: { name: string; state?: string }[];
}): boolean {
  const match = /^v(\d{4})\.(\d{2})\.(\d{2})\.([1-9]\d?)$/u.exec(release.tag_name);
  if (!match) return false;
  if (hasUploadedAsset(release, TAURI_RELEASE_ASSET)) return true;

  // Historical Tauri releases predate the runtime marker. This is a legacy
  // classification boundary, not a ceiling on future public Tauri updates.
  const [, year, month, day, revision] = match.map(Number);
  return year * 1_000_000 + month * 10_000 + day * 100 + revision <= 2026101104;
}
