import { afterEach, describe, expect, it, vi } from "vitest";

import {
  applyThumbnailNativeHover,
  clearThumbnailNativeHover,
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
    const elementFromPoint = vi.fn(() => card.classList.contains("thumbnail-card-native-active")
        ? button
        : image);
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: elementFromPoint,
    });

    expect(applyThumbnailNativeHover({ x: 40, y: 80, inside: true })).toBe(true);
    expect(card).toHaveClass("thumbnail-card-native-active");
    expect(button).toHaveClass("native-pointer-hover");
    expect(elementFromPoint).toHaveBeenCalledTimes(2);
  });

  it("clears native hover when the pointer leaves the preview", () => {
    document.body.innerHTML = `
      <article class="thumbnail-card thumbnail-card-native-active">
        <button class="native-pointer-hover">Copy</button>
      </article>
    `;

    expect(applyThumbnailNativeHover({ x: 0, y: 0, inside: false })).toBe(false);
    expect(document.querySelector(".thumbnail-card")).not.toHaveClass(
      "thumbnail-card-native-active",
    );
    expect(document.querySelector("button")).not.toHaveClass("native-pointer-hover");
  });

  it("keeps the active button interactive between polls", () => {
    document.body.innerHTML = `
      <article class="thumbnail-card thumbnail-card-native-active">
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
        if (!card.classList.contains("thumbnail-card-native-active")) becameInactive = true;
        return button;
      }),
    });

    expect(applyThumbnailNativeHover({ x: 40, y: 20, inside: true })).toBe(true);
    expect(becameInactive).toBe(false);
    expect(card).toHaveClass("thumbnail-card-native-active");
    expect(button).toHaveClass("native-pointer-hover");
  });
});

describe("clearThumbnailNativeHover", () => {
  it("clears both native hover classes", () => {
    document.body.innerHTML = `
      <article class="thumbnail-card-native-active">
        <button class="native-pointer-hover">Copy</button>
      </article>
    `;

    clearThumbnailNativeHover();

    expect(document.querySelector("article")).not.toHaveClass("thumbnail-card-native-active");
    expect(document.querySelector("button")).not.toHaveClass("native-pointer-hover");
  });
});
