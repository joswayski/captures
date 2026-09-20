export type ShareMetadata = {
  id: string;
  name: string;
  contentType: string;
  byteSize: number;
  passwordRequired: boolean;
  expiresAt: string | null;
  mediaUrl: string | null;
};
export type SharePageData =
  | { kind: "ready"; share: ShareMetadata }
  | { kind: "missing" }
  | { kind: "unavailable" };

export function mayIndex(_data: SharePageData): false {
  return false;
}

export function shareMediaKind(
  contentType: string,
): "image" | "video" | "download" {
  if (
    ["image/gif", "image/jpeg", "image/png", "image/webp"].includes(contentType)
  )
    return "image";
  if (["video/mp4", "video/webm", "video/ogg"].includes(contentType))
    return "video";
  return "download";
}
