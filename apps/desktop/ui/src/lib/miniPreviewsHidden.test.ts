import {
  buildMiniPreviewFolderDustParticles,
  miniPreviewFolderPlaceholderSheets,
  miniPreviewFolderSheets,
  miniPreviewsHiddenLabel,
  markMiniPreviewRestorePending,
  prepareMiniPreviewFolderMotion,
  takeMiniPreviewRestorePending,
  shouldIgnoreMiniPreviewsHiddenCursorEvents,
  MINI_PREVIEW_FOLDER_DUST_LEAD_MS,
  MINI_PREVIEW_FOLDER_SIZE_PX,
} from "./miniPreviewsHidden";

function rect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    x: left,
    y: top,
    left,
    top,
    right: left + width,
    bottom: top + height,
    width,
    height,
    toJSON: () => ({}),
  } as DOMRect;
}

describe("miniPreviewsHiddenLabel", () => {
  it("pluralizes parked preview counts", () => {
    expect(miniPreviewsHiddenLabel(0)).toBe("Previews");
    expect(miniPreviewsHiddenLabel(1)).toBe("1 preview");
    expect(miniPreviewsHiddenLabel(3)).toBe("3 previews");
  });
});

describe("miniPreviewFolderSheets", () => {
  it("keeps the newest captures in front of the pocket", () => {
    expect(miniPreviewFolderSheets([
      { id: "older", preview_url: "older.png" },
      { id: "newer", preview_url: "newer.png" },
    ])).toEqual([
      { id: "newer", src: "newer.png" },
      { id: "older", src: "older.png" },
    ]);
  });

  it("caps stacked sheets so the folder stays readable", () => {
    const sheets = miniPreviewFolderSheets(
      Array.from({ length: 6 }, (_, index) => ({
        id: `capture-${index}`,
        preview_url: `${index}.png`,
      })),
    );
    expect(sheets).toHaveLength(4);
    expect(sheets[0]?.id).toBe("capture-5");
  });

  it("builds empty placeholder sheets from a parked count", () => {
    expect(miniPreviewFolderPlaceholderSheets(3)).toEqual([
      { id: "preview-sheet-0", src: null },
      { id: "preview-sheet-1", src: null },
      { id: "preview-sheet-2", src: null },
    ]);
  });
});

describe("buildMiniPreviewFolderDustParticles", () => {
  it("uses the shared chip breakup from the folder's top-right contact point", () => {
    const particles = buildMiniPreviewFolderDustParticles(() => 0.5);

    expect(particles).toHaveLength(36);
    expect(particles.every(({ dy }) => dy < 0)).toBe(true);
    expect(particles.every(({ delayMs }) => (
      delayMs >= MINI_PREVIEW_FOLDER_DUST_LEAD_MS
    ))).toBe(true);

    const distanceFromContact = ({ sourceLeft, sourceTop, width, height }: typeof particles[number]) => (
      Math.hypot(
        sourceLeft + width / 2 - MINI_PREVIEW_FOLDER_SIZE_PX,
        sourceTop + height / 2,
      )
    );
    const nearest = particles.reduce((best, particle) => (
      distanceFromContact(particle) < distanceFromContact(best) ? particle : best
    ));
    const farthest = particles.reduce((best, particle) => (
      distanceFromContact(particle) > distanceFromContact(best) ? particle : best
    ));

    expect(nearest.delayMs).toBeLessThan(farthest.delayMs);
    expect(MINI_PREVIEW_FOLDER_DUST_LEAD_MS).toBe(370);
  });
});

describe("prepareMiniPreviewFolderMotion", () => {
  it("converges cards on the measured folder pocket", () => {
    document.body.innerHTML = `
      <main>
        <article class="thumbnail-card"></article>
        <article class="thumbnail-card"></article>
      </main>
      <span class="mini-preview-folder-icon"></span>
    `;
    const stack = document.querySelector("main")!;
    const cards = stack.querySelectorAll<HTMLElement>(".thumbnail-card");
    const folder = document.querySelector<HTMLElement>(".mini-preview-folder-icon")!;
    vi.spyOn(cards[0], "getBoundingClientRect").mockReturnValue(rect(28, 20, 284, 160));
    vi.spyOn(cards[1], "getBoundingClientRect").mockReturnValue(rect(28, 204, 284, 160));
    vi.spyOn(folder, "getBoundingClientRect").mockReturnValue(rect(14, 332, 28, 28));

    expect(prepareMiniPreviewFolderMotion(stack, folder)).toBe(2);
    expect(Number.parseFloat(cards[0].style.getPropertyValue("--thumbnail-folder-x")))
      .toBeCloseTo(3.8, 2);
    expect(Number.parseFloat(cards[0].style.getPropertyValue("--thumbnail-folder-y")))
      .toBeCloseTo(291.2, 2);
    expect(Number.parseFloat(cards[1].style.getPropertyValue("--thumbnail-folder-x")))
      .toBeCloseTo(6, 2);
    expect(Number.parseFloat(cards[1].style.getPropertyValue("--thumbnail-folder-y")))
      .toBeCloseTo(110, 2);
    expect(Number(cards[1].style.getPropertyValue("--thumbnail-folder-scale")))
      .toBeCloseTo(0.0775, 3);
    expect(cards[1].style.getPropertyValue("--thumbnail-folder-delay")).toBe("0ms");
  });

  it("ignores previews already leaving the stack", () => {
    document.body.innerHTML = `
      <main><article class="thumbnail-card thumbnail-exiting"></article></main>
      <span class="mini-preview-folder-icon"></span>
    `;

    expect(prepareMiniPreviewFolderMotion(
      document.querySelector("main")!,
      document.querySelector("span")!,
    )).toBe(0);
  });
});

describe("pending mini-preview restore", () => {
  afterEach(() => {
    takeMiniPreviewRestorePending();
  });

  it("starts the remounted stack in the open pose", () => {
    expect(takeMiniPreviewRestorePending()).toBe(false);
    markMiniPreviewRestorePending();
    expect(takeMiniPreviewRestorePending()).toBe(true);
    expect(takeMiniPreviewRestorePending()).toBe(false);
  });
});

describe("shouldIgnoreMiniPreviewsHiddenCursorEvents", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("keeps the visible chip interactive and passes the shadow gutter through", () => {
    document.body.innerHTML = `<button class="mini-previews-hidden">2 previews</button>`;
    const chip = document.querySelector(".mini-previews-hidden")!;
    vi.spyOn(chip, "getBoundingClientRect").mockReturnValue(rect(8, 8, 80, 56));

    expect(shouldIgnoreMiniPreviewsHiddenCursorEvents({ x: 20, y: 20, inside: true }))
      .toBe(false);
    expect(shouldIgnoreMiniPreviewsHiddenCursorEvents({ x: 4, y: 4, inside: true }))
      .toBe(true);
    expect(shouldIgnoreMiniPreviewsHiddenCursorEvents({ x: 0, y: 0, inside: false }))
      .toBe(false);
  });

  it("passes the whole restore window through while the chip is restoring", () => {
    document.body.innerHTML =
      `<button class="mini-previews-hidden mini-previews-hidden-restoring">2 previews</button>`;
    const chip = document.querySelector(".mini-previews-hidden")!;
    vi.spyOn(chip, "getBoundingClientRect").mockReturnValue(rect(8, 8, 80, 56));

    expect(shouldIgnoreMiniPreviewsHiddenCursorEvents({ x: 20, y: 20, inside: true }))
      .toBe(true);
    expect(shouldIgnoreMiniPreviewsHiddenCursorEvents({ x: 0, y: 0, inside: false }))
      .toBe(true);
  });
});
