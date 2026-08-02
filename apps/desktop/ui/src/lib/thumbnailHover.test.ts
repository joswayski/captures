import { afterEach, describe, expect, it, vi } from "vitest";

import {
  applyThumbnailCssCursor,
  applyThumbnailNativeHover,
  clearThumbnailCssCursor,
  clearThumbnailNativeHover,
  shouldIgnoreThumbnailCursorEvents,
  shouldRecoverThumbnailAfterNullPolls,
  thumbnailCssCursor,
  thumbnailCursorSyncAction,
  THUMBNAIL_CURSOR_KIND_ATTRIBUTE,
  THUMBNAIL_CURSOR_REASSERT_INTERVAL_MS,
  THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE,
  THUMBNAIL_NULL_POLL_RECOVER_COUNT,
  withThumbnailPointerTimeout,
} from "./thumbnailHover";

afterEach(() => {
  document.body.replaceChildren();
  Reflect.deleteProperty(document, "elementFromPoint");
  vi.restoreAllMocks();
});

function expectNativePointerHover(button: Element | null, hovered: boolean) {
  if (hovered) {
    expect(button).toHaveAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE, "true");
  } else {
    expect(button).not.toHaveAttribute(THUMBNAIL_NATIVE_POINTER_HOVER_ATTRIBUTE);
  }
}

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
    expectNativePointerHover(button, true);
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
    expectNativePointerHover(document.querySelector("button"), false);
  });

  it("clears native hover when the pointer leaves the preview", () => {
    document.body.innerHTML = `
      <article class="thumbnail-card" data-thumbnail-native-active="true">
        <button data-native-pointer-hover="true">Copy</button>
      </article>
    `;

    expect(applyThumbnailNativeHover({ x: 0, y: 0, inside: false })).toBe("default");
    expect(document.querySelector(".thumbnail-card"))
      .not.toHaveAttribute("data-thumbnail-native-active");
    expectNativePointerHover(document.querySelector("button"), false);
  });

  it("keeps the active button interactive between polls", () => {
    document.body.innerHTML = `
      <article class="thumbnail-card" data-thumbnail-native-active="true">
        <img alt="Screenshot preview">
        <button data-native-pointer-hover="true">Open Preview</button>
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
    expectNativePointerHover(button, true);
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
    expectNativePointerHover(button, true);
  });

  it("keeps pointer hover when React rewrites the button className", () => {
    document.body.innerHTML = `
      <article class="thumbnail-card">
        <button class="icon-button">Edit</button>
      </article>
    `;
    const button = document.querySelector<HTMLButtonElement>("button")!;
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => button),
    });

    expect(applyThumbnailNativeHover({ x: 40, y: 20, inside: true })).toBe("pointer");
    expectNativePointerHover(button, true);

    // IconButton re-renders set className from props and would wipe a class-based
    // hover marker. The data attribute must survive that write.
    button.className = "icon-button";
    expectNativePointerHover(button, true);
    expect(applyThumbnailNativeHover({ x: 40, y: 20, inside: true })).toBe("pointer");
  });

  it("keeps overflow cues clickable without activating a preview card", () => {
    document.body.innerHTML = `
      <button class="thumbnail-overflow-cue">Older captures</button>
      <article class="thumbnail-card" data-thumbnail-native-active="true">
        <button>Copy</button>
      </article>
    `;
    const overflowCue = document.querySelector<HTMLButtonElement>(
      ".thumbnail-overflow-cue",
    )!;
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => overflowCue),
    });

    expect(applyThumbnailNativeHover({ x: 40, y: 20, inside: true })).toBe("pointer");
    expectNativePointerHover(overflowCue, true);
    expect(document.querySelector(".thumbnail-card"))
      .not.toHaveAttribute("data-thumbnail-native-active");
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
    expectNativePointerHover(remainingButton, true);
  });
});

describe("clearThumbnailNativeHover", () => {
  it("clears the native card marker and button hover attribute", () => {
    document.body.innerHTML = `
      <article data-thumbnail-native-active="true">
        <button data-native-pointer-hover="true">Copy</button>
      </article>
    `;

    clearThumbnailNativeHover();

    expect(document.querySelector("article"))
      .not.toHaveAttribute("data-thumbnail-native-active");
    expectNativePointerHover(document.querySelector("button"), false);
  });
});

describe("shouldIgnoreThumbnailCursorEvents", () => {
  it("keeps live cards interactive and passes through empty or exiting regions", () => {
    document.body.innerHTML = `
      <main class="thumbnail-stack">
        <button class="thumbnail-overflow-cue">Older captures</button>
        <article id="live" class="thumbnail-card"><button>Copy</button></article>
        <article id="exiting" class="thumbnail-card thumbnail-exiting">
          <button>Delete</button>
        </article>
      </main>
    `;
    const stack = document.querySelector(".thumbnail-stack")!;
    const overflowCue = document.querySelector(".thumbnail-overflow-cue")!;
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

    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => overflowCue),
    });
    expect(shouldIgnoreThumbnailCursorEvents({ x: 10, y: 10, inside: true })).toBe(false);
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

  it("reasserts interactive cursors on every poll so macOS cannot flash the arrow", () => {
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
    // Negative elapsed is only used in tests; production always passes >= 0.
    expect(
      thumbnailCursorSyncAction(
        "pointer",
        "pointer",
        THUMBNAIL_CURSOR_REASSERT_INTERVAL_MS - 1,
      ),
    ).toBeNull();
  });

  it("force-reasserts interactive cursors for clicks and focus handoffs", () => {
    expect(
      thumbnailCursorSyncAction(
        "pointer",
        "pointer",
        0,
        { force: true },
      ),
    ).toBe("reassert");
    expect(
      thumbnailCursorSyncAction(
        "grab",
        "grab",
        0,
        { force: true },
      ),
    ).toBe("reassert");
    expect(
      thumbnailCursorSyncAction(
        "default",
        "default",
        0,
        { force: true },
      ),
    ).toBeNull();
  });

  it("does not reassert the default cursor", () => {
    expect(thumbnailCursorSyncAction("default", "default", Number.POSITIVE_INFINITY)).toBeNull();
  });
});

describe("thumbnailCssCursor", () => {
  it("maps cursor kinds to CSS values", () => {
    expect(thumbnailCssCursor("default")).toBe("default");
    expect(thumbnailCssCursor("pointer")).toBe("pointer");
    expect(thumbnailCssCursor("grab")).toBe("grab");
  });

  it("mirrors the hit-tested kind on the document without redundant writes", () => {
    applyThumbnailCssCursor("grab");
    expect(document.documentElement.style.cursor).toBe("grab");
    expect(document.documentElement).toHaveAttribute(
      THUMBNAIL_CURSOR_KIND_ATTRIBUTE,
      "grab",
    );

    // Same kind must not thrash style.cursor — WebKit treats each write as a
    // cursor-rectangle update and can flash the default arrow.
    const previous = document.documentElement.style.cursor;
    applyThumbnailCssCursor("grab");
    expect(document.documentElement.style.cursor).toBe(previous);

    applyThumbnailCssCursor("pointer");
    expect(document.documentElement.style.cursor).toBe("pointer");
    expect(document.documentElement).toHaveAttribute(
      THUMBNAIL_CURSOR_KIND_ATTRIBUTE,
      "pointer",
    );

    clearThumbnailCssCursor();
    expect(document.documentElement.style.cursor).toBe("");
    expect(document.documentElement).not.toHaveAttribute(THUMBNAIL_CURSOR_KIND_ATTRIBUTE);
  });
});

describe("thumbnail interactivity recovery helpers", () => {
  it("recovers only after a sustained run of empty pointer samples", () => {
    expect(shouldRecoverThumbnailAfterNullPolls(0)).toBe(false);
    expect(shouldRecoverThumbnailAfterNullPolls(THUMBNAIL_NULL_POLL_RECOVER_COUNT - 1)).toBe(false);
    expect(shouldRecoverThumbnailAfterNullPolls(THUMBNAIL_NULL_POLL_RECOVER_COUNT)).toBe(true);
  });

  it("times out hung pointer polls so sleep cannot stall the loop", async () => {
    const hung = new Promise<string>(() => undefined);
    const result = await withThumbnailPointerTimeout(hung, 20);
    expect(result).toBeNull();
  });

  it("resolves successful pointer polls before the timeout", async () => {
    const result = await withThumbnailPointerTimeout(
      Promise.resolve({ x: 1, y: 2, inside: true }),
      100,
    );
    expect(result).toEqual({ x: 1, y: 2, inside: true });
  });
});
