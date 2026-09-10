import {
  buildThumbnailDustParticles,
  coverBackgroundLayout,
  cubicBezierProgress,
  playThumbnailDustAnimations,
  playThumbnailDustCanvas,
  prefersReducedMotion,
  resetThumbnailDustCanvasPaintCache,
  THUMBNAIL_CARD_FALLBACK_HEIGHT,
  THUMBNAIL_CARD_FALLBACK_WIDTH,
  THUMBNAIL_DELETE_ORIGIN_X,
  THUMBNAIL_DELETE_ORIGIN_Y,
  THUMBNAIL_DISSOLVE_WAVE_MS,
  THUMBNAIL_DUST_LAYER_PAD_PX,
  thumbnailDeleteOriginX,
  thumbnailDustCanvasIsPaintable,
  thumbnailDustVisualAt,
} from "./thumbnailExit";
import type { ThumbnailDustParticle } from "./thumbnailExit";

const pad = THUMBNAIL_DUST_LAYER_PAD_PX;

afterEach(() => {
  vi.restoreAllMocks();
  resetThumbnailDustCanvasPaintCache();
});

describe("thumbnail exit effects", () => {
  it("mirrors delete dust origins with right-side controls", () => {
    expect(thumbnailDeleteOriginX(284, false, "left")).toBe(22.5);
    expect(thumbnailDeleteOriginX(284, true, "left")).toBe(57.5);
    expect(thumbnailDeleteOriginX(284, false, "right")).toBe(261.5);
    expect(thumbnailDeleteOriginX(284, true, "right")).toBe(226.5);
  });

  it("builds a grid of dust chips covering the card surface", () => {
    const particles = buildThumbnailDustParticles(140, 90, {
      cols: 4,
      rows: 3,
      random: () => 0.25,
      chromeLeadMs: 0,
    });

    expect(particles).toHaveLength(12);
    expect(particles[0]).toMatchObject({
      id: 0,
      left: pad,
      top: pad,
      cardWidth: 140,
      cardHeight: 90,
      sourceLeft: 0,
      sourceTop: 0,
      surfaceWidth: 140,
      surfaceHeight: 90,
      surfaceOffsetX: 0,
      surfaceOffsetY: 0,
    });
    expect(particles[particles.length - 1].left).toBeCloseTo(105 + pad);
    expect(particles[particles.length - 1].top).toBeCloseTo(60 + pad);
    for (const particle of particles) {
      expect(particle.width).toBeGreaterThan(0);
      expect(particle.height).toBeGreaterThan(0);
      expect(particle.durationMs).toBeGreaterThan(0);
    }
  });

  it("uses a dense-enough grid without overloading the compositor", () => {
    const particles = buildThumbnailDustParticles(
      THUMBNAIL_CARD_FALLBACK_WIDTH,
      THUMBNAIL_CARD_FALLBACK_HEIGHT,
      { random: () => 0.5 },
    );

    // Fine enough to read as dust, capped so WKWebView stays smooth.
    expect(particles.length).toBeGreaterThan(90);
    expect(particles.length).toBeLessThanOrEqual(220);
    const sample = particles[0];
    expect(sample.width).toBeLessThan(20);
    expect(sample.height).toBeLessThan(20);
  });

  it("keeps every corner cell as a slice of one full rounded card surface", () => {
    const width = 200;
    const height = 160;
    const cols = 14;
    const rows = 12;
    const particles = buildThumbnailDustParticles(width, height, {
      cols,
      rows,
      random: () => 0.5,
      chromeLeadMs: 0,
    });

    expect(particles).toHaveLength(cols * rows);
    expect(particles[0]).toMatchObject({
      cardWidth: width,
      cardHeight: height,
      sourceLeft: 0,
      sourceTop: 0,
    });
    expect(particles.at(-1)).toMatchObject({
      cardWidth: width,
      cardHeight: height,
      sourceLeft: (cols - 1) * (width / cols),
      sourceTop: (rows - 1) * (height / rows),
    });
  });

  it("matches object-fit: cover when sampling the preview into chips", () => {
    // Portrait image in a landscape card → cover crops top/bottom.
    const layout = coverBackgroundLayout(200, 100, 100, 200);
    expect(layout.surfaceWidth).toBeCloseTo(200);
    expect(layout.surfaceHeight).toBeCloseTo(400);
    expect(layout.offsetX).toBeCloseTo(0);
    expect(layout.offsetY).toBeCloseTo(-150);

    const particles = buildThumbnailDustParticles(200, 100, {
      cols: 2,
      rows: 2,
      random: () => 0,
      chromeLeadMs: 0,
      imageWidth: 100,
      imageHeight: 200,
    });
    expect(particles[0].surfaceWidth).toBeCloseTo(200);
    expect(particles[0].surfaceHeight).toBeCloseTo(400);
    expect(particles[0].surfaceOffsetY).toBeCloseTo(-150);
  });

  it("cascades particles radially from the trash button origin", () => {
    const originX = 40;
    const originY = 20;
    const particles = buildThumbnailDustParticles(200, 100, {
      cols: 5,
      rows: 5,
      random: () => 0.5,
      waveMs: 400,
      originX,
      originY,
      chromeLeadMs: 0,
    });

    const cardX = (p: { left: number; width: number }) => p.left + p.width / 2 - pad;
    const cardY = (p: { top: number; height: number }) => p.top + p.height / 2 - pad;

    const nearTrash = particles.reduce((best, p) => {
      const dist = Math.hypot(cardX(p) - originX, cardY(p) - originY);
      const bestDist = Math.hypot(cardX(best) - originX, cardY(best) - originY);
      return dist < bestDist ? p : best;
    });
    const farCorner = particles.reduce((best, p) => {
      const dist = Math.hypot(cardX(p) - originX, cardY(p) - originY);
      const bestDist = Math.hypot(cardX(best) - originX, cardY(best) - originY);
      return dist > bestDist ? p : best;
    });

    expect(nearTrash.delayMs).toBeLessThan(farCorner.delayMs);

    const midRing = particles.filter((p) => {
      const dist = Math.hypot(cardX(p) - originX, cardY(p) - originY);
      return dist > 45 && dist < 70;
    });
    expect(midRing.length).toBeGreaterThan(1);
    const delays = midRing.map((p) => p.delayMs);
    const spread = Math.max(...delays) - Math.min(...delays);
    expect(spread).toBeLessThan(220);
  });

  it("keeps the trash origin tight and adds more delay scatter farther out", () => {
    const originX = 20;
    const originY = 20;
    const particles = buildThumbnailDustParticles(200, 160, {
      cols: 12,
      rows: 10,
      random: () => 0.5,
      waveMs: 600,
      originX,
      originY,
      chromeLeadMs: 0,
    });

    const cardX = (p: { left: number; width: number }) => p.left + p.width / 2 - pad;
    const cardY = (p: { top: number; height: number }) => p.top + p.height / 2 - pad;

    const near = particles.filter((p) => {
      const dist = Math.hypot(cardX(p) - originX, cardY(p) - originY);
      return dist < 35;
    });
    const far = particles.filter((p) => {
      const dist = Math.hypot(cardX(p) - originX, cardY(p) - originY);
      return dist > 110;
    });
    expect(near.length).toBeGreaterThan(1);
    expect(far.length).toBeGreaterThan(1);

    const nearSpread = Math.max(...near.map((p) => p.delayMs)) - Math.min(...near.map((p) => p.delayMs));
    const farSpread = Math.max(...far.map((p) => p.delayMs)) - Math.min(...far.map((p) => p.delayMs));
    expect(nearSpread).toBeLessThan(farSpread);
    expect(nearSpread).toBeLessThan(90);
  });

  it("defaults the dissolve origin near the trash control", () => {
    const particles = buildThumbnailDustParticles(200, 100, {
      cols: 10,
      rows: 8,
      random: () => 0,
    });
    const cardX = (p: { left: number; width: number }) => p.left + p.width / 2 - pad;
    const cardY = (p: { top: number; height: number }) => p.top + p.height / 2 - pad;

    const nearest = particles.reduce((best, p) => {
      const dist = Math.hypot(cardX(p) - THUMBNAIL_DELETE_ORIGIN_X, cardY(p) - THUMBNAIL_DELETE_ORIGIN_Y);
      const bestDist = Math.hypot(cardX(best) - THUMBNAIL_DELETE_ORIGIN_X, cardY(best) - THUMBNAIL_DELETE_ORIGIN_Y);
      return dist < bestDist ? p : best;
    });
    const farthest = particles.reduce((best, p) => {
      const dist = Math.hypot(cardX(p) - THUMBNAIL_DELETE_ORIGIN_X, cardY(p) - THUMBNAIL_DELETE_ORIGIN_Y);
      const bestDist = Math.hypot(cardX(best) - THUMBNAIL_DELETE_ORIGIN_X, cardY(best) - THUMBNAIL_DELETE_ORIGIN_Y);
      return dist > bestDist ? p : best;
    });
    // Nearest chip to the trash control dissolves first.
    expect(nearest.delayMs).toBeLessThan(40);
    expect(nearest.delayMs).toBeLessThan(farthest.delayMs);
    expect(farthest.delayMs).toBeGreaterThan(THUMBNAIL_DISSOLVE_WAVE_MS * 0.7);
  });

  it("drifts particles only upward for an ash-like dissolve", () => {
    let i = 0;
    const sequence = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9];
    const particles = buildThumbnailDustParticles(
      THUMBNAIL_CARD_FALLBACK_WIDTH,
      THUMBNAIL_CARD_FALLBACK_HEIGHT,
      {
        cols: 5,
        rows: 4,
        random: () => sequence[i++ % sequence.length],
      },
    );

    const averageDy = particles.reduce((sum, p) => sum + p.dy, 0) / particles.length;
    expect(averageDy).toBeLessThan(-10);
    // Never positive Y — residual chips must not reverse into a drop.
    for (const particle of particles) {
      expect(particle.dy).toBeLessThan(0);
    }
  });

  it("spans roughly the shared dissolve-wave duration across the card", () => {
    const particles = buildThumbnailDustParticles(100, 100, {
      cols: 8,
      rows: 8,
      random: () => 0,
      originX: 0,
      originY: 0,
    });
    const delays = particles.map((p) => p.delayMs);
    expect(Math.min(...delays)).toBeLessThan(50);
    expect(Math.max(...delays)).toBeGreaterThan(THUMBNAIL_DISSOLVE_WAVE_MS * 0.75);
    // Outer angular wobble can push a bit past the base wave window.
    expect(Math.max(...delays)).toBeLessThan(THUMBNAIL_DISSOLVE_WAVE_MS * 1.25);
  });

  it("reads prefers-reduced-motion from the provided media query", () => {
    expect(prefersReducedMotion({ matches: true })).toBe(true);
    expect(prefersReducedMotion({ matches: false })).toBe(false);
    expect(prefersReducedMotion(null)).toBe(false);
  });

  it("plays explicit WAAPI keyframes per dust chip so WebView2 cannot drop motion", () => {
    const particles = buildThumbnailDustParticles(100, 80, {
      cols: 2,
      rows: 1,
      random: () => 0.5,
      chromeLeadMs: 0,
    });
    const animateCalls: Array<{ keyframes: Keyframe[]; options: KeyframeAnimationOptions }> = [];
    const chips = particles.map((particle) => {
      const el = document.createElement("span");
      el.animate = ((keyframes: Keyframe[] | PropertyIndexedKeyframes | null, options?: number | KeyframeAnimationOptions) => {
        animateCalls.push({
          keyframes: Array.isArray(keyframes) ? keyframes : [],
          options: typeof options === "object" && options ? options : {},
        });
        return {
          cancel: () => undefined,
        } as Animation;
      }) as typeof el.animate;
      // Keep particle identity available for assertions if needed.
      el.dataset.particleId = String(particle.id);
      return el;
    });

    const stop = playThumbnailDustAnimations(chips, particles);
    expect(animateCalls).toHaveLength(particles.length);

    // A paint-triggering property here multiplies work by ~200 chips/frame.
    for (const call of animateCalls) {
      for (const frame of call.keyframes) {
        expect(Object.keys(frame).every((key) => ["opacity", "transform", "offset"].includes(key)))
          .toBe(true);
      }
    }

    const first = animateCalls[0];
    expect(first.options.delay).toBe(particles[0].delayMs);
    expect(first.options.duration).toBe(particles[0].durationMs);
    expect(first.options.fill).toBe("forwards");

    // Endpoint transform must embed resolved px values (not CSS variables).
    const endFrame = first.keyframes[first.keyframes.length - 1];
    expect(String(endFrame.transform)).toContain(`${particles[0].dx}px`);
    expect(String(endFrame.transform)).toContain(`${particles[0].dy}px`);
    expect(String(endFrame.transform)).not.toContain("var(");

    expect(() => stop()).not.toThrow();
  });

  it("skips chips without Element.animate so tests and older hosts stay safe", () => {
    const particles = buildThumbnailDustParticles(60, 40, {
      cols: 2,
      rows: 1,
      random: () => 0.25,
    });
    const plain = document.createElement("span");
    // jsdom may still provide animate; remove it to model a host without WAAPI.
    // @ts-expect-error intentional host probe
    plain.animate = undefined;
    expect(() => playThumbnailDustAnimations([plain], particles)).not.toThrow();
  });

  it("eases the dissolve clock with the same cubic-bezier as WAAPI", () => {
    expect(cubicBezierProgress(0.28, 0, 0.12, 1, 0)).toBe(0);
    expect(cubicBezierProgress(0.28, 0, 0.12, 1, 1)).toBe(1);
    // At Bezier parameter 0.5: x = 0.105 + 0.045 + 0.125 = 0.275, y = 0.5.
    expect(cubicBezierProgress(0.28, 0, 0.12, 1, 0.275)).toBeCloseTo(0.5, 5);
  });

  it("keeps chips at rest until their delay, then fades them out by the duration", () => {
    const [particle] = buildThumbnailDustParticles(80, 40, {
      cols: 1,
      rows: 1,
      random: () => 0.5,
      chromeLeadMs: 0,
    });
    const rest = thumbnailDustVisualAt(particle, particle.delayMs);
    expect(rest.opacity).toBe(1);
    expect(rest.dx).toBe(0);
    expect(rest.dy).toBe(0);
    expect(rest.scale).toBe(1);

    const mid = thumbnailDustVisualAt(
      particle,
      particle.delayMs + particle.durationMs * 0.25,
    );
    expect(mid.opacity).toBeGreaterThan(0);
    expect(mid.opacity).toBeLessThan(1);
    expect(mid.dy).toBeLessThan(0);
    expect(mid.scale).toBeLessThan(1);

    const done = thumbnailDustVisualAt(
      particle,
      particle.delayMs + particle.durationMs,
    );
    expect(done.opacity).toBe(0);
    expect(done.dx).toBeCloseTo(particle.dx);
    expect(done.dy).toBeCloseTo(particle.dy);
    expect(done.scale).toBeCloseTo(0.18);
  });

  it("does not start a canvas dissolve when this host cannot paint pixels", () => {
    resetThumbnailDustCanvasPaintCache();
    expect(thumbnailDustCanvasIsPaintable()).toBe(false);
    const particles = buildThumbnailDustParticles(40, 20, {
      cols: 1,
      rows: 1,
      random: () => 0.5,
    });
    const canvas = document.createElement("canvas");
    const image = document.createElement("img");
    Object.defineProperty(image, "complete", { value: true });
    Object.defineProperty(image, "naturalWidth", { value: 40 });
    Object.defineProperty(image, "naturalHeight", { value: 20 });
    expect(playThumbnailDustCanvas(canvas, image, particles)).toBeNull();
  });
});

describe("canvas dust rendering", () => {
  // Deliberately asymmetric crop, chip size, delay, and flight. Expectations
  // below are independent of the random particle generator.
  const particle: ThumbnailDustParticle = {
    id: 0, left: 143, top: 127, width: 12.5, height: 8.5,
    cardWidth: 90, cardHeight: 50, sourceLeft: 23, sourceTop: 7,
    surfaceWidth: 200, surfaceHeight: 100, surfaceOffsetX: -55, surfaceOffsetY: -25,
    dx: 40, dy: -72, rotate: 30, delayMs: 60, durationMs: 1000,
  };

  function context() {
    return {
      filter: "none", fillStyle: "", globalAlpha: 1,
      fillRect: vi.fn(), getImageData: vi.fn(), drawImage: vi.fn(),
      setTransform: vi.fn(), beginPath: vi.fn(), roundRect: vi.fn(), clip: vi.fn(),
      save: vi.fn(), restore: vi.fn(), clearRect: vi.fn(),
      translate: vi.fn(), rotate: vi.fn(), scale: vi.fn(),
    };
  }

  function installCanvas() {
    const contexts = [context(), context(), context(), context(), context()];
    // Model a working synthetic filter probe. These are not capture pixels.
    contexts[0].getImageData
      .mockReturnValueOnce({ data: [127, 127, 127, 220] })
      .mockReturnValueOnce({ data: [127, 127, 127, 60] });
    let index = 0;
    const getContext = vi.spyOn(HTMLCanvasElement.prototype, "getContext")
      .mockImplementation(() => contexts[index++] as unknown as CanvasRenderingContext2D);
    const image = document.createElement("img");
    Object.defineProperties(image, {
      complete: { value: true }, naturalWidth: { value: 400 }, naturalHeight: { value: 200 },
    });
    const canvas = document.createElement("canvas");
    const frame = vi.fn<(callback: FrameRequestCallback) => number>(() => 7);
    const cancelFrame = vi.fn();
    return { contexts, getContext, canvas, image, frame, cancelFrame };
  }

  it.each(["missing filter", "ignored filter", "missing blur", "missing rounded clip"])(
    "falls back when basic canvas works but has %s", (failure) => {
      const { contexts, canvas, image, frame } = installCanvas();
      const probe = contexts[0];
      if (failure === "missing filter") Reflect.deleteProperty(probe, "filter");
      if (failure === "missing rounded clip") Reflect.deleteProperty(probe, "roundRect");
      if (failure === "ignored filter") {
        probe.getImageData.mockReset()
          .mockReturnValueOnce({ data: [255, 255, 255, 255] })
          .mockReturnValueOnce({ data: [0, 0, 0, 0] });
      }
      if (failure === "missing blur") {
        probe.getImageData.mockReset()
          .mockReturnValueOnce({ data: [127, 127, 127, 255] })
          .mockReturnValueOnce({ data: [0, 0, 0, 0] });
      }
      expect(playThumbnailDustCanvas(canvas, image, [particle], { frame })).toBeNull();
      expect(frame).not.toHaveBeenCalled();
      expect(contexts[1].drawImage).not.toHaveBeenCalled();
    },
  );

  it("caches the filter probe without reading capture pixels", () => {
    const { getContext, contexts } = installCanvas();
    expect(thumbnailDustCanvasIsPaintable()).toBe(true);
    expect(thumbnailDustCanvasIsPaintable()).toBe(true);
    expect(getContext).toHaveBeenCalledOnce();
    expect(contexts[0].filter).toBe("blur(1px) brightness(0.5)");
    expect(contexts[0].fillRect).toHaveBeenCalledWith(2, 2, 4, 4);
  });

  it.each([1, 2])("bakes clipped, padded chips once and draws immediately at DPR %s", (dpr) => {
    vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(dpr);
    const { contexts, canvas, image, frame, cancelFrame } = installCanvas();
    const stop = playThumbnailDustCanvas(canvas, image, [particle], { now: () => 100, frame, cancelFrame });
    expect(stop).not.toBeNull();
    const [, output, source, tiles, atlas] = contexts;
    expect(canvas.width).toBe(330 * dpr);
    expect(canvas.height).toBe(290 * dpr);
    expect(source.roundRect).toHaveBeenCalledWith(0, 0, 90, 50, 12);
    expect(source.drawImage).toHaveBeenCalledWith(image, -55, -25, 200, 100);
    expect(source.filter).toBe("none");
    expect(tiles.filter).toBe("none");
    expect(atlas.filter).toBe(`blur(${2 * dpr}px) brightness(0.5)`);
    expect(tiles.drawImage).toHaveBeenCalledWith(expect.any(HTMLCanvasElement),
      23 * dpr, 7 * dpr, 12.5 * dpr, 8.5 * dpr,
      8 * dpr, 8 * dpr, 12.5 * dpr, 8.5 * dpr);
    expect(atlas.drawImage).toHaveBeenCalledWith(expect.any(HTMLCanvasElement), 0, 0);
    expect(output.translate).toHaveBeenCalledWith(149.25, 131.25);
    expect(output.drawImage).toHaveBeenCalledWith(expect.any(HTMLCanvasElement),
      0, 0, Math.ceil(28.5 * dpr), Math.ceil(24.5 * dpr),
      -14.25, -12.25, Math.ceil(28.5 * dpr) / dpr, Math.ceil(24.5 * dpr) / dpr);

    // elapsed 335 - delay 60 = 275ms: eased progress 0.5, opacity 0.72.
    frame.mock.calls[0][0](435);
    expect(output.globalAlpha).toBeCloseTo(0.72, 5);
    expect(output.scale.mock.lastCall?.[0]).toBeCloseTo(0.645116279, 5);
    // Bezier inversion has 1e-6 time tolerance; allow sub-millipixel pose error.
    expect(output.translate.mock.lastCall?.[0]).toBeCloseTo(167.38953488, 3);
    expect(output.translate.mock.lastCall?.[1]).toBeCloseTo(98.59883721, 3);
    expect(source.drawImage).toHaveBeenCalledOnce();
    expect(tiles.drawImage).toHaveBeenCalledOnce();
    expect(atlas.drawImage).toHaveBeenCalledOnce();
    for (const ctx of [output, source, tiles, atlas]) expect(ctx.getImageData).not.toHaveBeenCalled();
    stop!();
    expect(cancelFrame).toHaveBeenCalledWith(7);
    const draws = output.drawImage.mock.calls.length;
    frame.mock.lastCall![0](500);
    expect(output.drawImage).toHaveBeenCalledTimes(draws);
  });

  it("keeps later atlas rows separate from earlier chips", () => {
    vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(2);
    const { contexts, canvas, image, frame } = installCanvas();
    const particles = [particle, { ...particle, id: 1 }, { ...particle, id: 2, sourceLeft: 3, sourceTop: 30 }];
    const stop = playThumbnailDustCanvas(canvas, image, particles, { frame });
    expect(contexts[3].drawImage).toHaveBeenNthCalledWith(3, expect.any(HTMLCanvasElement),
      6, 60, 25, 17, 16, 65, 25, 17);
    // The expensive filter operation runs once, not once per particle.
    expect(contexts[3].filter).toBe("none");
    expect(contexts[4].drawImage).toHaveBeenCalledOnce();
    expect(contexts[1].drawImage).toHaveBeenNthCalledWith(3, expect.any(HTMLCanvasElement),
      0, 49, 57, 49, -14.25, -12.25, 28.5, 24.5);
    stop!();
  });

  it("clears the last frame and stops as soon as all chips are transparent", () => {
    const { contexts, canvas, image, frame, cancelFrame } = installCanvas();
    const stop = playThumbnailDustCanvas(canvas, image, [particle], { now: () => 0, frame, cancelFrame });
    frame.mock.calls[0][0](900); // opacity is already zero, duration has not ended.
    expect(contexts[1].clearRect).toHaveBeenCalledTimes(2);
    expect(contexts[1].drawImage).toHaveBeenCalledOnce();
    expect(frame).toHaveBeenCalledOnce();
    stop!();
    expect(cancelFrame).not.toHaveBeenCalled();
  });

  it.each([1, 2, 3, 4])("falls back if drawing context %s fails to draw", (index) => {
    const { contexts, canvas, image, frame } = installCanvas();
    contexts[index].drawImage.mockImplementation(() => { throw new Error("draw unavailable"); });
    expect(playThumbnailDustCanvas(canvas, image, [particle], { frame })).toBeNull();
    expect(frame).not.toHaveBeenCalled();
  });
});
