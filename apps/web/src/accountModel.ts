export const MAX_UPLOAD_BYTES = 20 * 1024 * 1024;
export const IMAGE_TYPES = ["image/png", "image/jpeg", "image/webp"] as const;

export function validateUpload(
  file: Pick<File, "size" | "type">,
): string | null {
  if (!IMAGE_TYPES.includes(file.type as (typeof IMAGE_TYPES)[number])) {
    return "Choose a static PNG, JPEG, or WebP image.";
  }
  if (file.size > MAX_UPLOAD_BYTES) return "Images must be 20 MiB or smaller.";
  if (file.size === 0) return "That image is empty.";
  return null;
}

export function validateShare(
  password: string,
  expiresAt: string,
): string | null {
  if (password && (password.length < 8 || password.length > 128)) {
    return "Passwords must be between 8 and 128 characters.";
  }
  if (expiresAt && (!Number.isFinite(new Date(expiresAt).getTime()) || new Date(expiresAt).getTime() <= Date.now())) {
    return "Expiry must be in the future.";
  }
  return null;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024 * 1024)
    return `${Math.max(1, Math.round(bytes / 1024))} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}
