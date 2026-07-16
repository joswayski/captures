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
