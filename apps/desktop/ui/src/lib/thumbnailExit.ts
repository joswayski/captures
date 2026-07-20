/** Logical size of a thumbnail card used when the live element cannot be measured. */
export const THUMBNAIL_CARD_FALLBACK_WIDTH = 284;
export const THUMBNAIL_CARD_FALLBACK_HEIGHT = 160;

export type ThumbnailDustParticle = {
  id: number;
  left: number;
  top: number;
  width: number;
  height: number;
  surfaceWidth: number;
  surfaceHeight: number;
  bgX: number;
  bgY: number;
  dx: number;
  dy: number;
  rotate: number;
  delayMs: number;
  durationMs: number;
};

export function prefersReducedMotion(
  media?: Pick<MediaQueryList, "matches"> | null,
): boolean {
  const query = media === undefined
    ? typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia("(prefers-reduced-motion: reduce)")
      : null
    : media;
  return Boolean(query?.matches);
}

/**
 * Slice a card into a grid of image chips that drift away as dust.
 * Uses CSS background-position (not canvas) so custom-protocol previews work
 * without tainting or pixel-read restrictions.
 */
export function buildThumbnailDustParticles(
  cardWidth: number,
  cardHeight: number,
  options?: {
    cols?: number;
    rows?: number;
    random?: () => number;
  },
): ThumbnailDustParticle[] {
  const width = Math.max(1, cardWidth);
  const height = Math.max(1, cardHeight);
  const cols = options?.cols ?? 14;
  const rows = options?.rows ?? 9;
  const random = options?.random ?? Math.random;
  const cellW = width / cols;
  const cellH = height / rows;
  const particles: ThumbnailDustParticle[] = [];
  let id = 0;

  for (let row = 0; row < rows; row += 1) {
    for (let col = 0; col < cols; col += 1) {
      const left = col * cellW;
      const top = row * cellH;
      // Drift mostly upward and outward — ash-like, not a uniform explode.
      const angle = -Math.PI / 2 + (random() - 0.5) * 1.7;
      const distance = 24 + random() * 78;
      const cascade = (row / rows) * 70 + (col / cols) * 35;

      particles.push({
        id: id++,
        left,
        top,
        // Slight overlap hides sub-pixel gaps between chips.
        width: cellW + 0.6,
        height: cellH + 0.6,
        surfaceWidth: width,
        surfaceHeight: height,
        bgX: left === 0 ? 0 : -left,
        bgY: top === 0 ? 0 : -top,
        dx: Math.cos(angle) * distance + (random() - 0.5) * 18,
        dy: Math.sin(angle) * distance - 12 - random() * 36,
        rotate: (random() - 0.5) * 90,
        delayMs: Math.floor(random() * 90 + cascade),
        durationMs: 500 + Math.floor(random() * 220),
      });
    }
  }

  return particles;
}
