// Build-time generator for the thumbnail delete "dust" animation. Ported from
// the retired Tauri UI; `prepare.mjs` writes its particles and sampled poses to
// `dust.json`/`poses.json`, which every native host and its tests read.
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
 * ~10–12px keeps the dissolve fine without drawing hundreds of tiny chips.
 */
export const THUMBNAIL_DUST_TARGET_CELL_PX = 11;

/** Soft cap so a large card cannot spawn more chips than the GPU likes. */
export const THUMBNAIL_DUST_MAX_PARTICLES = 220;

/**
 * Padding around the dust layer so a hover-matched blur filter does not cage
 * chips inside the card.
 */
export const THUMBNAIL_DUST_LAYER_PAD_PX = 120;

export type ThumbnailDustParticle = {
  id: number;
  left: number;
  top: number;
  width: number;
  height: number;
  cardWidth: number;
  cardHeight: number;
  sourceLeft: number;
  sourceTop: number;
  surfaceWidth: number;
  surfaceHeight: number;
  surfaceOffsetX: number;
  surfaceOffsetY: number;
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
 * Supplies matching source coordinates to the canvas and CSS-background
 * renderers; neither renderer reads capture pixels.
 *
 * Every grid cell is retained. Rendering each cell as a clipped slice of one
 * full-size rounded surface preserves the exact corner arc without removing
 * whole corner chips inward or revealing square corner stubs during flight.
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
  // Prefer a modest grid; hard-cap so large cards stay smooth.
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
    Math.hypot(Math.max(originX, width - originX), Math.max(originY, height - originY)),
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
      // full-size until chips finish.
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
        cardWidth: width,
        cardHeight: height,
        sourceLeft: left,
        sourceTop: top,
        surfaceWidth: cover.surfaceWidth,
        surfaceHeight: cover.surfaceHeight,
        // One full-card surface is offset beneath each clipped particle cell.
        surfaceOffsetX: cover.offsetX,
        surfaceOffsetY: cover.offsetY,
        dx,
        dy,
        rotate: (random() - 0.5) * 120,
        delayMs: Math.floor(chromeLeadMs + delayNorm * waveMs + delayJitterMs),
        // Longer flight + soft ease-out reads smoother.
        durationMs: 780 + Math.floor(random() * 320 + wave * 80),
      });
    }
  }

  return particles;
}

/** CSS `cubic-bezier(0.28, 0, 0.12, 1)`, the dust flight easing. */
export const THUMBNAIL_DUST_EASE = {
  x1: 0.28,
  y1: 0,
  x2: 0.12,
  y2: 1,
} as const;

const THUMBNAIL_DUST_TRANSFORM_LIFT_AT = 0.14;
const THUMBNAIL_DUST_OPACITY_KEYS = [
  { at: 0, value: 1 },
  { at: THUMBNAIL_DUST_TRANSFORM_LIFT_AT, value: 1 },
  { at: 0.5, value: 0.72 },
  { at: 0.82, value: 0 },
  { at: 1, value: 0 },
] as const;

export type ThumbnailDustVisual = {
  opacity: number;
  dx: number;
  dy: number;
  rotate: number;
  scale: number;
};

function lerp(start: number, end: number, t: number): number {
  return start + (end - start) * t;
}

function sampleCubicBezier(t: number, a: number, b: number): number {
  const inverse = 1 - t;
  return 3 * inverse * inverse * t * a + 3 * inverse * t * t * b + t * t * t;
}

function sampleCubicBezierDerivative(t: number, a: number, b: number): number {
  const inverse = 1 - t;
  return 3 * inverse * inverse * a + 6 * inverse * t * (b - a) + 3 * t * t * (1 - b);
}

/**
 * CSS `cubic-bezier` y for x in [0, 1]. Newton with a binary-search fallback
 * so a near-flat tangent cannot stall the dissolve clock.
 */
export function cubicBezierProgress(
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  x: number,
): number {
  if (x <= 0) return 0;
  if (x >= 1) return 1;
  let t = x;
  for (let step = 0; step < 8; step += 1) {
    const currentX = sampleCubicBezier(t, x1, x2);
    const delta = currentX - x;
    if (Math.abs(delta) < 1e-6) {
      return sampleCubicBezier(t, y1, y2);
    }
    const derivative = sampleCubicBezierDerivative(t, x1, x2);
    if (Math.abs(derivative) < 1e-6) break;
    t = Math.min(1, Math.max(0, t - delta / derivative));
  }
  let low = 0;
  let high = 1;
  t = x;
  for (let step = 0; step < 12; step += 1) {
    const currentX = sampleCubicBezier(t, x1, x2);
    if (currentX < x) low = t;
    else high = t;
    t = (low + high) / 2;
  }
  return sampleCubicBezier(t, y1, y2);
}

function lerpStepped(keys: readonly { at: number; value: number }[], t: number): number {
  const first = keys[0];
  const last = keys[keys.length - 1];
  if (!first || !last || t <= first.at) return first?.value ?? 0;
  if (t >= last.at) return last.value;
  for (let index = 1; index < keys.length; index += 1) {
    const next = keys[index];
    const previous = keys[index - 1];
    if (!next || !previous || t > next.at) continue;
    const span = next.at - previous.at;
    return lerp(previous.value, next.value, span <= 0 ? 1 : (t - previous.at) / span);
  }
  return last.value;
}

/**
 * Visual pose at `elapsedMs` from the dissolve start, matching the shipped
 * keyframes (easing on the whole duration, then linear keyframe mix).
 */
export function thumbnailDustVisualAt(
  particle: ThumbnailDustParticle,
  elapsedMs: number,
): ThumbnailDustVisual {
  const localMs = elapsedMs - particle.delayMs;
  if (localMs <= 0) {
    return { opacity: 1, dx: 0, dy: 0, rotate: 0, scale: 1 };
  }
  const linear = Math.min(1, localMs / Math.max(1, particle.durationMs));
  const t = cubicBezierProgress(
    THUMBNAIL_DUST_EASE.x1,
    THUMBNAIL_DUST_EASE.y1,
    THUMBNAIL_DUST_EASE.x2,
    THUMBNAIL_DUST_EASE.y2,
    linear,
  );
  const opacity = lerpStepped(THUMBNAIL_DUST_OPACITY_KEYS, t);
  const lift = THUMBNAIL_DUST_TRANSFORM_LIFT_AT;
  if (t <= lift) {
    const mix = lift <= 0 ? 1 : t / lift;
    return {
      opacity,
      dx: lerp(0, particle.dx * 0.06, mix),
      dy: lerp(0, particle.dy * 0.06, mix),
      rotate: lerp(0, particle.rotate * 0.08, mix),
      scale: lerp(1, 0.98, mix),
    };
  }
  const mix = (t - lift) / (1 - lift);
  return {
    opacity,
    dx: lerp(particle.dx * 0.06, particle.dx, mix),
    dy: lerp(particle.dy * 0.06, particle.dy, mix),
    rotate: lerp(particle.rotate * 0.08, particle.rotate, mix),
    scale: lerp(0.98, 0.18, mix),
  };
}
