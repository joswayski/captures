import {
  buildThumbnailDustParticles,
  coverBackgroundLayout,
  prefersReducedMotion,
  THUMBNAIL_CARD_FALLBACK_HEIGHT,
  THUMBNAIL_CARD_FALLBACK_WIDTH,
  THUMBNAIL_DELETE_ORIGIN_X,
  THUMBNAIL_DELETE_ORIGIN_Y,
  THUMBNAIL_DISSOLVE_WAVE_MS,
  THUMBNAIL_DUST_LAYER_PAD_PX,
} from "./thumbnailExit";

const pad = THUMBNAIL_DUST_LAYER_PAD_PX;

describe("thumbnail exit effects", () => {
  it("builds a full grid of dust chips covering the card surface", () => {
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
      surfaceWidth: 140,
      surfaceHeight: 90,
      bgX: 0,
      bgY: 0,
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
    expect(particles.length).toBeGreaterThan(100);
    expect(particles.length).toBeLessThanOrEqual(220);
    const sample = particles[0];
    expect(sample.width).toBeLessThan(20);
    expect(sample.height).toBeLessThan(20);
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
    expect(particles[0].bgY).toBeCloseTo(-150);
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

  it("drifts particles mostly upward for an ash-like dissolve", () => {
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
});
