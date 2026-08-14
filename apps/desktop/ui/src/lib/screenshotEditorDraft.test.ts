import {
  buildScreenshotEditorDraftPayload,
  collectDocumentImageSources,
  draftAssetRef,
  hydrateScreenshotEditorDraftDocument,
  isScreenshotDocumentDirty,
  parseDraftAssetRef,
  rewriteDocumentImageSources,
  SCREENSHOT_EDITOR_DRAFT_MAX_TOTAL_BYTES,
} from "./screenshotEditorDraft";
import { createScreenshotDocument, type ScreenshotDocument } from "./screenshotEditor";

function sampleDocument(): ScreenshotDocument {
  return {
    width: 100,
    height: 80,
    background: "#f7f7f5",
    elements: [
      {
        id: "capture-background",
        kind: "image",
        source: "background",
        src: "captures-capture://localhost/artifact-full/a",
        originalSrc: null,
        name: "Original",
        sourceArtifactId: "a",
        x: 0,
        y: 0,
        width: 100,
        height: 80,
        naturalWidth: 100,
        naturalHeight: 80,
        locked: true,
        visible: true,
        opacity: 100,
        blendMode: "source-over",
      },
      {
        id: "note",
        kind: "text",
        text: "hello",
        fontSize: 24,
        width: 80,
        fontFamily: "sans",
        bold: false,
        italic: false,
        align: "left",
        color: "#111",
        background: null,
        outlined: false,
        roundedBackground: false,
        x: 10,
        y: 10,
        locked: false,
        visible: true,
        opacity: 100,
        blendMode: "source-over",
      },
    ],
  };
}

describe("screenshot editor drafts", () => {
  it("detects dirty documents against a baseline", () => {
    const base = createScreenshotDocument("a", 10, 10, "id");
    expect(isScreenshotDocumentDirty(base, base)).toBe(false);
    expect(isScreenshotDocumentDirty({ ...base, background: null }, base)).toBe(true);
    expect(isScreenshotDocumentDirty(base, null)).toBe(true);
  });

  it("collects unique image sources including originalSrc", () => {
    const document = sampleDocument();
    document.elements[0] = {
      ...document.elements[0],
      kind: "image",
      originalSrc: "data:image/png;base64,abc",
    } as ScreenshotDocument["elements"][0];
    expect(collectDocumentImageSources(document)).toEqual([
      "captures-capture://localhost/artifact-full/a",
      "data:image/png;base64,abc",
    ]);
  });

  it("parses and builds draft asset refs", () => {
    expect(draftAssetRef("asset-1")).toBe("draft-asset:asset-1");
    expect(parseDraftAssetRef("draft-asset:asset-1")).toBe("asset-1");
    expect(parseDraftAssetRef("captures-capture://x")).toBeNull();
  });

  it("rewrites image sources while leaving vectors alone", () => {
    const rewritten = rewriteDocumentImageSources(sampleDocument(), (src) => `x:${src}`);
    const image = rewritten.elements.find((element) => element.kind === "image");
    const text = rewritten.elements.find((element) => element.kind === "text");
    expect(image && image.kind === "image" && image.src).toBe(
      "x:captures-capture://localhost/artifact-full/a",
    );
    expect(text && text.kind === "text" && text.text).toBe("hello");
  });

  it("builds a draft payload with asset refs and PNG bytes", async () => {
    let n = 0;
    const payload = await buildScreenshotEditorDraftPayload(
      "capture-1",
      sampleDocument(),
      async () => [1, 2, 3],
      1_700_000_000_000,
      () => `asset-${++n}`,
    );
    expect(payload.artifact_id).toBe("capture-1");
    expect(payload.updated_at_ms).toBe(1_700_000_000_000);
    expect(payload.assets).toEqual([{ id: "asset-1", png: [1, 2, 3] }]);
    const image = payload.document.elements.find((element) => element.kind === "image");
    expect(image && image.kind === "image" && image.src).toBe("draft-asset:asset-1");
  });

  it("hydrates draft asset refs into protocol URLs", () => {
    const draftDoc = rewriteDocumentImageSources(sampleDocument(), () => draftAssetRef("a1"));
    const hydrated = hydrateScreenshotEditorDraftDocument(
      draftDoc,
      (id) => `captures-capture://localhost/editor-draft/capture-1/${id}`,
    );
    const image = hydrated.elements.find((element) => element.kind === "image");
    expect(image && image.kind === "image" && image.src).toBe(
      "captures-capture://localhost/editor-draft/capture-1/a1",
    );
  });

  it("rejects drafts that exceed the total asset budget", async () => {
    // Sparse array so the test checks the byte cap without allocating 80MB+.
    const oversized: number[] = [];
    oversized.length = SCREENSHOT_EDITOR_DRAFT_MAX_TOTAL_BYTES + 1;
    await expect(
      buildScreenshotEditorDraftPayload(
        "capture-1",
        sampleDocument(),
        async () => oversized,
      ),
    ).rejects.toThrow(/too large/i);
  });
});
