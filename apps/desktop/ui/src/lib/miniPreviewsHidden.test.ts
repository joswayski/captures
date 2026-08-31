import {
  miniPreviewsHiddenLabel,
  prepareMiniPreviewFolderMotion,
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

describe("prepareMiniPreviewFolderMotion", () => {
  it("converges cards on the measured bottom-left folder anchor", () => {
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
    expect(cards[0].style.getPropertyValue("--thumbnail-folder-x")).toBe("-14.5px");
    expect(cards[0].style.getPropertyValue("--thumbnail-folder-y")).toBe("309.5px");
    expect(cards[1].style.getPropertyValue("--thumbnail-folder-x")).toBe("-13px");
    expect(cards[1].style.getPropertyValue("--thumbnail-folder-y")).toBe("127px");
    expect(Number(cards[1].style.getPropertyValue("--thumbnail-folder-scale")))
      .toBeCloseTo(0.1056, 3);
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
