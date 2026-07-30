/** Logical size of a thumbnail card used when the live element cannot be measured. */
export const THUMBNAIL_CARD_FALLBACK_WIDTH = 284;
export const THUMBNAIL_CARD_FALLBACK_HEIGHT = 160;

/**
 * How long the disintegration front takes to travel from the trash control
 * to the farthest corner of the card.
 */
export const THUMBNAIL_DISSOLVE_WAVE_MS = 720;

/**
 * Extra delay before dust starts. Kept near zero so chrome dissolves in place
 * with the ash wave instead of a slow "wait for buttons" phase.
 */
export const THUMBNAIL_CHROME_LEAD_MS = 0;

/**
 * Center of the delete control relative to the card origin.
 * top/left padding 8px + half of the 29px icon button.
 * Before a folder save, delete is the first control; after save it sits next to Close.
 */
export const THUMBNAIL_DELETE_ORIGIN_FIRST_X = 22.5;
export const THUMBNAIL_DELETE_ORIGIN_AFTER_CLOSE_X = 57.5; // 8 + 29 + 6 + 14.5
export const THUMBNAIL_DELETE_ORIGIN_X = THUMBNAIL_DELETE_ORIGIN_AFTER_CLOSE_X;
export const THUMBNAIL_DELETE_ORIGIN_Y = 22.5;

/**
 * Target chip size in CSS pixels.
 * ~10–12px keeps the dissolve fine without flooding the compositor
 * (hundreds of filter-animating DOM nodes tank FPS in WKWebView).
 */
export const THUMBNAIL_DUST_TARGET_CELL_PX = 11;

/** Soft cap so a large card cannot spawn more chips than the GPU likes. */
export const THUMBNAIL_DUST_MAX_PARTICLES = 220;

/**
 * Padding around the dust layer so a hover-matched blur filter does not cage
 * chips inside the card. Must match `--dust-pad` in styles.css.
 */
export const THUMBNAIL_DUST_LAYER_PAD_PX = 120;

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

export type CoverBackgroundLayout = {
  surfaceWidth: number;
  surfaceHeight: number;
  offsetX: number;
  offsetY: number;
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

function gridCount(size: number, targetCell: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Math.round(size / targetCell)));
}

/**
 * Match CSS `object-fit: cover` so dust chips sample the same crop as the
 * thumbnail `<img>` (no letterbox bars, no stretch mismatch on dissolve).
 */
export function coverBackgroundLayout(
  cardWidth: number,
  cardHeight: number,
  imageWidth: number,
  imageHeight: number,
): CoverBackgroundLayout {
  const width = Math.max(1, cardWidth);
  const height = Math.max(1, cardHeight);
  const imgW = Math.max(1, imageWidth);
  const imgH = Math.max(1, imageHeight);
  const scale = Math.max(width / imgW, height / imgH);
  const surfaceWidth = imgW * scale;
  const surfaceHeight = imgH * scale;
  return {
    surfaceWidth,
    surfaceHeight,
    offsetX: (width - surfaceWidth) / 2,
    offsetY: (height - surfaceHeight) / 2,
  };
}

/**
 * Slice a card into a fine grid of image chips that dissolve as ash/dust.
 * Delay starts at the trash button and expands radially; after the front
 * leaves the origin, delays get progressively more irregular so the wave
 * reads organic instead of a perfect circle.
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
    waveMs?: number;
    originX?: number;
    originY?: number;
    targetCellPx?: number;
    imageWidth?: number;
    imageHeight?: number;
    chromeLeadMs?: number;
  },
): ThumbnailDustParticle[] {
  const width = Math.max(1, cardWidth);
  const height = Math.max(1, cardHeight);
  const targetCell = options?.targetCellPx ?? THUMBNAIL_DUST_TARGET_CELL_PX;
  // Prefer a modest grid; hard-cap so large cards stay smooth in WKWebView.
  let cols = options?.cols ?? gridCount(width, targetCell, 14, 24);
  let rows = options?.rows ?? gridCount(height, targetCell, 8, 16);
  if (cols * rows > THUMBNAIL_DUST_MAX_PARTICLES) {
    const scale = Math.sqrt(THUMBNAIL_DUST_MAX_PARTICLES / (cols * rows));
    cols = Math.max(10, Math.floor(cols * scale));
    rows = Math.max(6, Math.floor(rows * scale));
  }
  const random = options?.random ?? Math.random;
  const waveMs = options?.waveMs ?? THUMBNAIL_DISSOLVE_WAVE_MS;
  const chromeLeadMs = options?.chromeLeadMs ?? THUMBNAIL_CHROME_LEAD_MS;
  const originX = options?.originX ?? Math.min(THUMBNAIL_DELETE_ORIGIN_X, width * 0.35);
  const originY = options?.originY ?? Math.min(THUMBNAIL_DELETE_ORIGIN_Y, height * 0.3);
  const cover = coverBackgroundLayout(
    width,
    height,
    options?.imageWidth ?? width,
    options?.imageHeight ?? height,
  );
  const cellW = width / cols;
  const cellH = height / rows;
  const pad = THUMBNAIL_DUST_LAYER_PAD_PX;
  // Farthest corner from the trash origin — normalizes radial delays to [0, 1].
  const maxDist = Math.max(
    1,
    Math.hypot(
      Math.max(originX, width - originX),
      Math.max(originY, height - originY),
    ),
  );
  const particles: ThumbnailDustParticle[] = [];
  let id = 0;

  for (let row = 0; row < rows; row += 1) {
    for (let col = 0; col < cols; col += 1) {
      const left = col * cellW;
      const top = row * cellH;
      const cx = left + cellW / 2;
      const cy = top + cellH / 2;
      const dist = Math.hypot(cx - originX, cy - originY);
      // 0 at trash → 1 at the farthest corner.
      const wave = dist / maxDist;
      // Near the trash the front stays tight; further out it gets ragged.
      const angle = Math.atan2(cy - originY, cx - originX);
      const angularWobble = Math.sin(angle * 2.7 + wave * 5.5) * 0.07 * wave;
      const scatter = (random() - 0.5) * 0.34 * wave * wave;
      const delayNorm = Math.min(1.12, Math.max(0, wave + angularWobble + scatter));
      // Extra ms-jitter grows with distance so outer chips pop less in lockstep.
      const delayJitterMs = random() * (18 + wave * 140);
      // Ash drifts upward/outward only — never downward. Card layout stays
      // full-size until chips finish (see thumbnail-delete hold in styles.css).
      const awayX = (cx - originX) / maxDist;
      const dx = awayX * (12 + random() * 26) + (random() - 0.5) * 22;
      const dy = -36 - random() * 58;

      particles.push({
        id: id++,
        // Offset into the padded dust layer so positions still map to the card.
        left: left + pad,
        top: top + pad,
        // Slight overlap hides sub-pixel gaps between chips.
        width: cellW + 0.55,
        height: cellH + 0.55,
        surfaceWidth: cover.surfaceWidth,
        surfaceHeight: cover.surfaceHeight,
        // Chip-local background position matching object-fit: cover on the card.
        bgX: cover.offsetX - left,
        bgY: cover.offsetY - top,
        dx,
        dy,
        rotate: (random() - 0.5) * 120,
        delayMs: Math.floor(chromeLeadMs + delayNorm * waveMs + delayJitterMs),
        // Longer flight + soft ease-out reads smoother in WKWebView.
        durationMs: 780 + Math.floor(random() * 320 + wave * 80),
      });
    }
  }

  return particles;
}

/**
 * Drive dust chip flight with the Web Animations API.
 *
 * CSS `@keyframes` that read per-chip custom properties (`--dust-x`, etc.) are
 * unreliable in Windows WebView2: transforms often stay at rest while only the
 * parent layer / source image opacity fades — reading as a plain dissolve.
 * Explicit WAAPI keyframes with resolved pixel values animate consistently on
 * WebView2, WKWebView, and WebKitGTK.
 */
export function playThumbnailDustAnimations(
  chips: ArrayLike<Element>,
  particles: readonly ThumbnailDustParticle[],
): () => void {
  const animations: Animation[] = [];
  const count = Math.min(chips.length, particles.length);
  for (let index = 0; index < count; index += 1) {
    const chip = chips[index];
    const particle = particles[index];
    if (!(chip instanceof HTMLElement) || !particle) continue;

    // Some jsdom/test environments stub Element.animate.
    if (typeof chip.animate !== "function") continue;

    const animation = chip.animate(
      [
        {
          opacity: 1,
          transform: "translate3d(0, 0, 0) rotate(0deg) scale(1)",
          boxShadow: "none",
          offset: 0,
        },
        {
          opacity: 1,
          transform: `translate3d(${particle.dx * 0.06}px, ${particle.dy * 0.06}px, 0) rotate(${particle.rotate * 0.08}deg) scale(0.98)`,
          boxShadow: "0 0 0 0.5px rgba(0, 0, 0, .08), 0 1px 3px rgba(0, 0, 0, .14)",
          offset: 0.14,
        },
        {
          opacity: 0.72,
          offset: 0.5,
        },
        {
          opacity: 0,
          offset: 0.82,
        },
        {
          opacity: 0,
          transform: `translate3d(${particle.dx}px, ${particle.dy}px, 0) rotate(${particle.rotate}deg) scale(0.18)`,
          boxShadow: "none",
          offset: 1,
        },
      ],
      {
        duration: particle.durationMs,
        delay: particle.delayMs,
        easing: "cubic-bezier(0.28, 0, 0.12, 1)",
        fill: "forwards",
      },
    );
    animations.push(animation);
  }

  return () => {
    for (const animation of animations) {
      try {
        animation.cancel();
      } catch {
        // Ignore already-finished animations.
      }
    }
  };
}
