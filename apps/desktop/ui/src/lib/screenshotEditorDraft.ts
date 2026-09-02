import type { ScreenshotDocument, ScreenshotElement } from "./screenshotEditor";

/** Prefix for image `src` values stored inside a draft manifest. */
export const DRAFT_ASSET_PREFIX = "draft-asset:";

export const SCREENSHOT_EDITOR_DRAFT_SCHEMA_VERSION = 1;

/** Debounce before writing a dirty document to disk. */
export const SCREENSHOT_EDITOR_DRAFT_SAVE_MS = 700;

/**
 * Max time the editor window may stay open after the user clicks close while a
 * draft flush is in progress. Encoding full-resolution PNG layers to `number[]`
 * and shipping them over IPC can take seconds; Tauri only destroys the window
 * after `onCloseRequested` resolves, so an uncapped await made the red X look
 * completely dead. Autosave already keeps most sessions on disk; this budget
 * only covers a last best-effort write.
 */
export const SCREENSHOT_EDITOR_DRAFT_CLOSE_FLUSH_MS = 400;

/** Soft cap so huge multi-layer edits do not fill the disk unexpectedly. */
export const SCREENSHOT_EDITOR_DRAFT_MAX_TOTAL_BYTES = 80 * 1024 * 1024;

/**
 * One image layer in a draft save. `png` is `null` when the backend already
 * holds this asset from an earlier save of the same draft, so the bytes do not
 * need to be re-encoded or shipped over IPC again.
 */
export type ScreenshotEditorDraftAsset = {
  id: string;
  png: number[] | null;
};

/** Backend error returned when a `png: null` asset no longer exists on disk. */
export const SCREENSHOT_EDITOR_DRAFT_ASSET_MISSING =
  "a previously saved draft image asset is missing; resend the full draft";

export function isDraftAssetMissingError(reason: unknown): boolean {
  return String(reason).includes(SCREENSHOT_EDITOR_DRAFT_ASSET_MISSING);
}

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
 *
 * `createAssetId(src)` should return a stable id for a given `src` across saves
 * so `isPersisted(assetId)` can skip re-encoding layers the backend already has;
 * those assets are sent with `png: null`.
 */
export async function buildScreenshotEditorDraftPayload(
  artifactId: string,
  document: ScreenshotDocument,
  encodePng: (src: string) => Promise<number[]>,
  nowMs: number = Date.now(),
  createAssetId: (src: string) => string = () => crypto.randomUUID(),
  isPersisted: (assetId: string) => boolean = () => false,
): Promise<ScreenshotEditorDraftPayload> {
  const sources = collectDocumentImageSources(document);
  const assets: ScreenshotEditorDraftAsset[] = [];
  const sourceToAsset = new Map<string, string>();
  let totalBytes = 0;

  for (const src of sources) {
    if (sourceToAsset.has(src)) continue;
    const existing = parseDraftAssetRef(src);
    const assetId = existing ?? createAssetId(src);
    if (isPersisted(assetId)) {
      assets.push({ id: assetId, png: null });
      sourceToAsset.set(src, assetId);
      continue;
    }
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
