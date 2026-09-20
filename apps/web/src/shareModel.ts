export type ShareMetadata = {
  id: string;
  visibility: "private" | "unlisted" | "public";
  passwordRequired: boolean;
  expiresAt: string | null;
  mediaUrl: string | null;
};
export type SharePageData =
  | { kind: "ready"; share: ShareMetadata }
  | { kind: "missing" }
  | { kind: "unavailable" };

export function mayIndex(data: SharePageData): boolean {
  return (
    data.kind === "ready" &&
    data.share.visibility === "public" &&
    !data.share.passwordRequired &&
    Boolean(data.share.mediaUrl)
  );
}
