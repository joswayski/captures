/** Runtime identity written only by the validated public Tauri release job. */
export const TAURI_RELEASE_ASSET = "captures-tauri.json";

export function isTauriPreview(release: {
  tag_name: string;
  assets?: { name: string; state?: string }[];
}): boolean {
  const match = /^v(\d{4})\.(\d{2})\.(\d{2})\.([1-9]\d?)$/u.exec(release.tag_name);
  if (!match) return false;
  if (
    release.assets?.some(
      (asset) => asset.name === TAURI_RELEASE_ASSET && asset.state === "uploaded",
    )
  )
    return true;

  // Historical Tauri releases predate the runtime marker. This is a legacy
  // classification boundary, not a ceiling on future public Tauri updates.
  const [, year, month, day, revision] = match.map(Number);
  return year * 1_000_000 + month * 10_000 + day * 100 + revision <= 2026101104;
}
