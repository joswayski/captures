const FILE_SIZE_UNITS = ["B", "KB", "MB", "GB"] as const;

export function formatFileSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";

  let value = bytes;
  let unitIndex = 0;
  while (value >= 1_000 && unitIndex < FILE_SIZE_UNITS.length - 1) {
    value /= 1_000;
    unitIndex += 1;
  }

  if (unitIndex === 0) return `${Math.round(value)} ${FILE_SIZE_UNITS[unitIndex]}`;
  const precision = value >= 100 ? 0 : 1;
  return `${value.toFixed(precision)} ${FILE_SIZE_UNITS[unitIndex]}`;
}

/**
 * Byte count Est. size % should compare against.
 *
 * Pass the flattened source from the current estimate first, then the Before
 * badge, so a compact file on disk, a previous quality preset, or a stale
 * preview after a failed refresh is not treated as the original.
 */
export function fileSizeDeltaBaseline(
  originalFileBytes: number,
  sourceImageBytes: number | null,
): number {
  if (sourceImageBytes !== null && sourceImageBytes > 0) {
    return sourceImageBytes;
  }
  return originalFileBytes;
}

/**
 * Percent change from the original image to an estimated export, or null when
 * the change is unknown or rounds to zero.
 */
export function formatFileSizeDelta(
  estimatedBytes: number | null,
  originalBytes: number,
): { percent: number; label: string } | null {
  if (estimatedBytes === null || originalBytes <= 0) return null;
  const percent = Math.round((estimatedBytes / originalBytes - 1) * 100);
  if (percent === 0) return null;
  return {
    percent,
    label: percent < 0 ? `−${Math.abs(percent)}%` : `+${percent}%`,
  };
}
