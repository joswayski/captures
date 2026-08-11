import type { ScreenshotDocument, ScreenshotElement } from "./screenshotEditor";

/** Prefix for image `src` values stored inside a draft manifest. */
export const DRAFT_ASSET_PREFIX = "draft-asset:";

export const SCREENSHOT_EDITOR_DRAFT_SCHEMA_VERSION = 1;

/** Debounce before writing a dirty document to disk. */
export const SCREENSHOT_EDITOR_DRAFT_SAVE_MS = 700;

/** Soft cap so huge multi-layer edits do not fill the disk unexpectedly. */
export const SCREENSHOT_EDITOR_DRAFT_MAX_TOTAL_BYTES = 80 * 1024 * 1024;

export type ScreenshotEditorDraftAsset = {
  id: string;
  png: number[];
};

export type ScreenshotEditorDraftPayload = {
  artifact_id: string;
  document: ScreenshotDocument;
  assets: ScreenshotEditorDraftAsset[];
  updated_at_ms: number;
};

export type LoadedScreenshotEditorDraft = {
  document: ScreenshotDocument;
  updated_at_ms: number;
};

export function isScreenshotDocumentDirty(
  document: ScreenshotDocument,
  baseline: ScreenshotDocument | null,
): boolean {
  if (!baseline) return true;
  return JSON.stringify(document) !== JSON.stringify(baseline);
}

export function draftAssetRef(assetId: string): string {
  return `${DRAFT_ASSET_PREFIX}${assetId}`;
}

export function parseDraftAssetRef(src: string): string | null {
  if (!src.startsWith(DRAFT_ASSET_PREFIX)) return null;
  const id = src.slice(DRAFT_ASSET_PREFIX.length);
  return id.length > 0 ? id : null;
}

/** Collect unique image bitmap URLs from a document (`src` + `originalSrc`). */
export function collectDocumentImageSources(document: ScreenshotDocument): string[] {
  const sources = new Set<string>();
  for (const element of document.elements) {
    if (element.kind !== "image") continue;
    if (element.src) sources.add(element.src);
    if (element.originalSrc) sources.add(element.originalSrc);
  }
  return [...sources];
}

export function rewriteDocumentImageSources(
  document: ScreenshotDocument,
  rewrite: (src: string) => string,
): ScreenshotDocument {
  return {
    ...document,
    elements: document.elements.map((element): ScreenshotElement => {
      if (element.kind !== "image") return element;
      return {
        ...element,
        src: rewrite(element.src),
        originalSrc: element.originalSrc ? rewrite(element.originalSrc) : element.originalSrc,
      };
    }),
  };
}

/**
 * Build a disk-ready draft: rewrite every image URL to a `draft-asset:` ref and
 * pair those refs with PNG bytes produced by `encodePng(src)`.
 */
export async function buildScreenshotEditorDraftPayload(
  artifactId: string,
  document: ScreenshotDocument,
  encodePng: (src: string) => Promise<number[]>,
  nowMs: number = Date.now(),
  createAssetId: () => string = () => crypto.randomUUID(),
): Promise<ScreenshotEditorDraftPayload> {
  const sources = collectDocumentImageSources(document);
  const assets: ScreenshotEditorDraftAsset[] = [];
  const sourceToAsset = new Map<string, string>();
  let totalBytes = 0;

  for (const src of sources) {
    if (sourceToAsset.has(src)) continue;
    const existing = parseDraftAssetRef(src);
    const assetId = existing ?? createAssetId();
    const png = await encodePng(src);
    totalBytes += png.length;
    if (totalBytes > SCREENSHOT_EDITOR_DRAFT_MAX_TOTAL_BYTES) {
      throw new Error(
        "Unsaved edits are too large to keep as a draft. Save a file before closing.",
      );
    }
    assets.push({ id: assetId, png });
    sourceToAsset.set(src, assetId);
  }

  const draftDocument = rewriteDocumentImageSources(document, (src) => {
    const assetId = sourceToAsset.get(src);
    return assetId ? draftAssetRef(assetId) : src;
  });

  return {
    artifact_id: artifactId,
    document: draftDocument,
    assets,
    updated_at_ms: nowMs,
  };
}

/**
 * After load, map `draft-asset:` refs to protocol URLs the webview can paint.
 * `assetUrl(assetId)` should return a captures-capture editor-draft URL.
 */
export function hydrateScreenshotEditorDraftDocument(
  document: ScreenshotDocument,
  assetUrl: (assetId: string) => string,
): ScreenshotDocument {
  return rewriteDocumentImageSources(document, (src) => {
    const assetId = parseDraftAssetRef(src);
    return assetId ? assetUrl(assetId) : src;
  });
}
