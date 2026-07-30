import { afterEach, describe, expect, it, vi } from "vitest";

import {
  applyThumbnailNativeHover,
  clearThumbnailNativeHover,
  shouldIgnoreThumbnailCursorEvents,
  thumbnailCssCursor,
  thumbnailCursorSyncAction,
  THUMBNAIL_CURSOR_REASSERT_INTERVAL_MS,
} from "./thumbnailHover";

afterEach(() => {
  document.body.replaceChildren();
  Reflect.deleteProperty(document, "elementFromPoint");
  vi.restoreAllMocks();
});

describe("applyThumbnailNativeHover", () => {
  it("activates the card before hit-testing its buttons", () => {
    document.body.innerHTML = `
      <article class="thumbnail-card">
        <img alt="Screenshot preview">
        <div class="thumbnail-main-actions"><button>Copy</button></div>
      </article>
    `;
    const card = document.querySelector<HTMLElement>(".thumbnail-card")!;
    const image = document.querySelector<HTMLImageElement>("img")!;
    const button = document.querySelector<HTMLButtonElement>("button")!;
    const elementFromPoint = vi.fn(() => card.hasAttribute("data-thumbnail-native-active")
        ? button
        : image);
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: elementFromPoint,
    });

    expect(applyThumbnailNativeHover({ x: 40, y: 80, inside: true })).toBe("pointer");
    expect(card).toHaveAttribute("data-thumbnail-native-active", "true");
    expect(button).toHaveClass("native-pointer-hover");
    expect(elementFromPoint).toHaveBeenCalledTimes(2);
  });

  it("uses a grab cursor over the preview image so file drag is obvious", () => {
    document.body.innerHTML = `
      <article class="thumbnail-card">
        <img alt="Screenshot preview">
        <div class="thumbnail-main-actions"><button>Copy</button></div>
      </article>
    `;
    const image = document.querySelector<HTMLImageElement>("img")!;
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => image),
    });

    expect(applyThumbnailNativeHover({ x: 40, y: 80, inside: true })).toBe("grab");
    expect(document.querySelector(".thumbnail-card"))
      .toHaveAttribute("data-thumbnail-native-active", "true");
    expect(document.querySelector("button")).not.toHaveClass("native-pointer-hover");
  });

  it("clears native hover when the pointer leaves the preview", () => {
    document.body.innerHTML = `
      <article class="thumbnail-card" data-thumbnail-native-active="true">
        <button class="native-pointer-hover">Copy</button>
      </article>
    `;

    expect(applyThumbnailNativeHover({ x: 0, y: 0, inside: false })).toBe("default");
    expect(document.querySelector(".thumbnail-card"))
      .not.toHaveAttribute("data-thumbnail-native-active");
    expect(document.querySelector("button")).not.toHaveClass("native-pointer-hover");
  });

  it("keeps the active button interactive between polls", () => {
    document.body.innerHTML = `
      <article class="thumbnail-card" data-thumbnail-native-active="true">
        <img alt="Screenshot preview">
        <button class="native-pointer-hover">Open Preview</button>
      </article>
    `;
    const card = document.querySelector<HTMLElement>(".thumbnail-card")!;
    const button = document.querySelector<HTMLButtonElement>("button")!;
    let becameInactive = false;
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => {
        if (!card.hasAttribute("data-thumbnail-native-active")) becameInactive = true;
        return button;
      }),
    });

    expect(applyThumbnailNativeHover({ x: 40, y: 20, inside: true })).toBe("pointer");
    expect(becameInactive).toBe(false);
    expect(card).toHaveAttribute("data-thumbnail-native-active", "true");
    expect(button).toHaveClass("native-pointer-hover");
  });

  it("retains the pointing cursor through a transient WebKit focus handoff", () => {
    document.body.innerHTML = `
      <article class="thumbnail-card">
        <img alt="Screenshot preview">
        <button>Edit</button>
      </article>
    `;
    const image = document.querySelector<HTMLImageElement>("img")!;
    const button = document.querySelector<HTMLButtonElement>("button")!;
    vi.spyOn(button, "getBoundingClientRect").mockReturnValue({
      x: 20,
      y: 10,
      top: 10,
      left: 20,
      right: 80,
      bottom: 50,
      width: 60,
      height: 40,
      toJSON: () => ({}),
    });
    let handoff = false;
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => handoff ? image : button),
    });

    expect(applyThumbnailNativeHover({ x: 40, y: 20, inside: true })).toBe("pointer");
    handoff = true;
    expect(applyThumbnailNativeHover({ x: 40, y: 20, inside: true })).toBe("pointer");
    expect(button).toHaveClass("native-pointer-hover");
  });

  it("moves hover directly to a remaining card after the stack changes", () => {
    document.body.innerHTML = `
      <article id="removed" class="thumbnail-card"><button>Delete</button></article>
      <article id="remaining" class="thumbnail-card"><button>Open Preview</button></article>
    `;
    const removed = document.querySelector<HTMLElement>("#removed")!;
    const removedButton = removed.querySelector<HTMLButtonElement>("button")!;
    const remaining = document.querySelector<HTMLElement>("#remaining")!;
    const remainingButton = remaining.querySelector<HTMLButtonElement>("button")!;
    let target: Element = removedButton;
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => target),
    });

    expect(applyThumbnailNativeHover({ x: 40, y: 20, inside: true })).toBe("pointer");
    removed.remove();
    target = remainingButton;
    expect(applyThumbnailNativeHover({ x: 40, y: 20, inside: true })).toBe("pointer");

    expect(remaining).toHaveAttribute("data-thumbnail-native-active", "true");
    expect(remainingButton).toHaveClass("native-pointer-hover");
  });
});

describe("clearThumbnailNativeHover", () => {
  it("clears the native card marker and button class", () => {
    document.body.innerHTML = `
      <article data-thumbnail-native-active="true">
        <button class="native-pointer-hover">Copy</button>
      </article>
    `;

    clearThumbnailNativeHover();

    expect(document.querySelector("article"))
      .not.toHaveAttribute("data-thumbnail-native-active");
    expect(document.querySelector("button")).not.toHaveClass("native-pointer-hover");
  });
});

describe("shouldIgnoreThumbnailCursorEvents", () => {
  it("keeps live cards interactive and passes through empty or exiting regions", () => {
    document.body.innerHTML = `
      <main class="thumbnail-stack">
        <article id="live" class="thumbnail-card"><button>Copy</button></article>
        <article id="exiting" class="thumbnail-card thumbnail-exiting">
          <button>Delete</button>
        </article>
      </main>
    `;
    const stack = document.querySelector(".thumbnail-stack")!;
    const live = document.querySelector("#live")!;
    const exiting = document.querySelector("#exiting")!;
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => live),
    });
    expect(shouldIgnoreThumbnailCursorEvents({ x: 10, y: 10, inside: true })).toBe(false);

    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => exiting),
    });
    expect(shouldIgnoreThumbnailCursorEvents({ x: 10, y: 10, inside: true })).toBe(true);

    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => stack),
    });
    expect(shouldIgnoreThumbnailCursorEvents({ x: 10, y: 10, inside: true })).toBe(true);
    expect(shouldIgnoreThumbnailCursorEvents({ x: 10, y: 10, inside: false })).toBe(false);
  });
});

describe("thumbnailCursorSyncAction", () => {
  it("syncs cursor transitions immediately", () => {
    expect(thumbnailCursorSyncAction("default", "pointer", 0)).toBe("transition");
    expect(thumbnailCursorSyncAction("pointer", "default", 0)).toBe("transition");
    expect(thumbnailCursorSyncAction("default", "grab", 0)).toBe("transition");
    expect(thumbnailCursorSyncAction("grab", "pointer", 0)).toBe("transition");
  });

  it("periodically reasserts interactive cursors that macOS may have reset", () => {
    expect(
      thumbnailCursorSyncAction(
        "pointer",
        "pointer",
        THUMBNAIL_CURSOR_REASSERT_INTERVAL_MS - 1,
      ),
    ).toBeNull();
    expect(
      thumbnailCursorSyncAction(
        "pointer",
        "pointer",
        THUMBNAIL_CURSOR_REASSERT_INTERVAL_MS,
      ),
    ).toBe("reassert");
    expect(
      thumbnailCursorSyncAction(
        "grab",
        "grab",
        THUMBNAIL_CURSOR_REASSERT_INTERVAL_MS,
      ),
    ).toBe("reassert");
  });

  it("does not reassert the default cursor", () => {
    expect(thumbnailCursorSyncAction("default", "default", Number.POSITIVE_INFINITY)).toBeNull();
  });
});

describe("thumbnailCssCursor", () => {
  it("maps cursor kinds to CSS values", () => {
    expect(thumbnailCssCursor("default")).toBe("");
    expect(thumbnailCssCursor("pointer")).toBe("pointer");
    expect(thumbnailCssCursor("grab")).toBe("grab");
  });
});
