import {
  buildThumbnailDustParticles,
  prefersReducedMotion,
  THUMBNAIL_CARD_FALLBACK_HEIGHT,
  THUMBNAIL_CARD_FALLBACK_WIDTH,
} from "./thumbnailExit";

describe("thumbnail exit effects", () => {
  it("builds a full grid of dust chips covering the card surface", () => {
    const particles = buildThumbnailDustParticles(140, 90, {
      cols: 4,
      rows: 3,
      random: () => 0.25,
    });

    expect(particles).toHaveLength(12);
    expect(particles[0]).toMatchObject({
      id: 0,
      left: 0,
      top: 0,
      surfaceWidth: 140,
      surfaceHeight: 90,
      bgX: 0,
      bgY: 0,
    });
    expect(particles[particles.length - 1].left).toBeCloseTo(105);
    expect(particles[particles.length - 1].top).toBeCloseTo(60);
    for (const particle of particles) {
      expect(particle.width).toBeGreaterThan(0);
      expect(particle.height).toBeGreaterThan(0);
      expect(particle.durationMs).toBeGreaterThan(0);
    }
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

  it("reads prefers-reduced-motion from the provided media query", () => {
    expect(prefersReducedMotion({ matches: true })).toBe(true);
    expect(prefersReducedMotion({ matches: false })).toBe(false);
    expect(prefersReducedMotion(null)).toBe(false);
  });
});
