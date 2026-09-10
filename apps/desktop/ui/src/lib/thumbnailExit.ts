/** Logical size of a thumbnail card used when the live element cannot be measured. */
export const THUMBNAIL_CARD_FALLBACK_WIDTH = 284;
export const THUMBNAIL_CARD_FALLBACK_HEIGHT = 160;

/**
 * Card / preview corner radius in CSS pixels.
 * Must match `.thumbnail-card` / `.thumbnail-media` / dust clip in styles.css.
 */
export const THUMBNAIL_CARD_BORDER_RADIUS_PX = 12;

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

/** Center the delete dissolve on the mirrored control when the stack is right-anchored. */
export function thumbnailDeleteOriginX(
  cardWidth: number,
  hasFolderFile: boolean,
  side: "left" | "right",
): number {
  const leftOrigin = hasFolderFile
    ? THUMBNAIL_DELETE_ORIGIN_AFTER_CLOSE_X
    : THUMBNAIL_DELETE_ORIGIN_FIRST_X;
  return side === "right" ? cardWidth - leftOrigin : leftOrigin;
}

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

    // Keep flight compositor-only. Interpolating a tiny box-shadow on every
    // chip repaints the filtered image slices every frame; the existing blur
    // and opacity already soften their edges without that extra paint work.
    const animation = chip.animate(
      [
        {
          opacity: 1,
          transform: "translate3d(0, 0, 0) rotate(0deg) scale(1)",
          offset: 0,
        },
        {
          opacity: 1,
          transform: `translate3d(${particle.dx * 0.06}px, ${particle.dy * 0.06}px, 0) rotate(${particle.rotate * 0.08}deg) scale(0.98)`,
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

/** Matches `playThumbnailDustAnimations` / CSS `cubic-bezier(0.28, 0, 0.12, 1)`. */
export const THUMBNAIL_DUST_EASE = {
  x1: 0.28,
  y1: 0,
  x2: 0.12,
  y2: 1,
} as const;

/** Same filter as `.thumbnail-dust` so the canvas path matches the DOM fallback. */
export const THUMBNAIL_DUST_FILTER = "blur(2px) brightness(0.5)";

/**
 * Extra source pixels so a 2px Gaussian blur is not clipped at the card edge
 * before chips are sliced.
 */
const THUMBNAIL_DUST_SOURCE_BLUR_PAD_PX = 8;

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

function lerpStepped(
  keys: readonly { at: number; value: number }[],
  t: number,
): number {
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
 * Visual pose at `elapsedMs` from the dissolve start, matching the WAAPI
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

let cachedDustCanvasPaintable: boolean | null = null;

/** Test hook so jsdom and real-canvas probes do not leak across cases. */
export function resetThumbnailDustCanvasPaintCache(): void {
  cachedDustCanvasPaintable = null;
}

/**
 * True when this document can display a 2D canvas. jsdom's stub context does
 * not round-trip pixels, so delete still mounts the DOM-chip fallback there.
 * A later production canvas may be tainted by `captures-capture://`; that is
 * fine because we only paint, never read back.
 */
export function thumbnailDustCanvasIsPaintable(): boolean {
  if (cachedDustCanvasPaintable !== null) return cachedDustCanvasPaintable;
  if (typeof document === "undefined") {
    cachedDustCanvasPaintable = false;
    return false;
  }
  try {
    const probe = document.createElement("canvas");
    probe.width = 2;
    probe.height = 2;
    const context = probe.getContext("2d");
    if (!context || typeof context.drawImage !== "function") {
      cachedDustCanvasPaintable = false;
      return false;
    }
    context.fillStyle = "#010203";
    context.fillRect(0, 0, 2, 2);
    const pixel = context.getImageData(0, 0, 1, 1).data;
    cachedDustCanvasPaintable = pixel[0] === 1 && pixel[1] === 2 && pixel[2] === 3;
  } catch {
    cachedDustCanvasPaintable = false;
  }
  return cachedDustCanvasPaintable;
}

function dustSourceIsReady(image: CanvasImageSource): boolean {
  if (image instanceof HTMLImageElement) {
    return image.complete && image.naturalWidth > 0 && image.naturalHeight > 0;
  }
  if (typeof HTMLCanvasElement !== "undefined" && image instanceof HTMLCanvasElement) {
    return image.width > 0 && image.height > 0;
  }
  return true;
}

function clipDustSourceRoundedRect(
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  radius: number,
) {
  const nextRadius = Math.max(0, Math.min(radius, width / 2, height / 2));
  context.beginPath();
  if (typeof context.roundRect === "function") {
    context.roundRect(x, y, width, height, nextRadius);
  } else {
    context.rect(x, y, width, height);
  }
  context.clip();
}

function paintThumbnailDustSource(
  context: CanvasRenderingContext2D,
  image: CanvasImageSource,
  particle: ThumbnailDustParticle,
  blurPad: number,
) {
  context.save();
  clipDustSourceRoundedRect(
    context,
    blurPad,
    blurPad,
    particle.cardWidth,
    particle.cardHeight,
    THUMBNAIL_CARD_BORDER_RADIUS_PX,
  );
  context.filter = THUMBNAIL_DUST_FILTER;
  context.drawImage(
    image,
    blurPad + particle.surfaceOffsetX,
    blurPad + particle.surfaceOffsetY,
    particle.surfaceWidth,
    particle.surfaceHeight,
  );
  context.restore();
}

function devicePixelRatioNow(): number {
  if (typeof window === "undefined" || !Number.isFinite(window.devicePixelRatio)) {
    return 1;
  }
  return Math.max(1, window.devicePixelRatio);
}

/**
 * Paint the dissolve on one canvas instead of hundreds of filter-animating
 * DOM chips. Custom-protocol previews may taint the bitmap; we never read
 * pixels from it. Returns `null` when this host cannot paint (jsdom, a
 * missing 2D context, or an unloaded image) so the caller can mount chips.
 */
export function playThumbnailDustCanvas(
  canvas: HTMLCanvasElement,
  image: CanvasImageSource,
  particles: readonly ThumbnailDustParticle[],
  options: {
    now?: () => number;
    frame?: (callback: FrameRequestCallback) => number;
    cancelFrame?: (id: number) => void;
  } = {},
): (() => void) | null {
  if (particles.length === 0 || !dustSourceIsReady(image) || !thumbnailDustCanvasIsPaintable()) {
    return null;
  }
  const context = canvas.getContext("2d", { alpha: true });
  if (!context) return null;

  const sample = particles[0];
  if (!sample) return null;
  const pad = THUMBNAIL_DUST_LAYER_PAD_PX;
  const blurPad = THUMBNAIL_DUST_SOURCE_BLUR_PAD_PX;
  const cssWidth = sample.cardWidth + pad * 2;
  const cssHeight = sample.cardHeight + pad * 2;
  const dpr = devicePixelRatioNow();
  canvas.style.width = `${cssWidth}px`;
  canvas.style.height = `${cssHeight}px`;
  canvas.width = Math.max(1, Math.round(cssWidth * dpr));
  canvas.height = Math.max(1, Math.round(cssHeight * dpr));

  const source = document.createElement("canvas");
  const sourceCssWidth = sample.cardWidth + blurPad * 2;
  const sourceCssHeight = sample.cardHeight + blurPad * 2;
  source.width = Math.max(1, Math.round(sourceCssWidth * dpr));
  source.height = Math.max(1, Math.round(sourceCssHeight * dpr));
  const sourceContext = source.getContext("2d", { alpha: true });
  if (!sourceContext) return null;
  sourceContext.setTransform(dpr, 0, 0, dpr, 0, 0);
  try {
    paintThumbnailDustSource(sourceContext, image, sample, blurPad);
  } catch {
    // Custom-protocol images can throw on drawImage in some WebViews.
    return null;
  }

  const sourceScaleX = source.width / sourceCssWidth;
  const sourceScaleY = source.height / sourceCssHeight;
  const now = options.now ?? (() => performance.now());
  const frame = options.frame
    ?? ((callback: FrameRequestCallback) => requestAnimationFrame(callback));
  const cancelFrame = options.cancelFrame
    ?? ((id: number) => cancelAnimationFrame(id));
  const startedAt = now();
  let frameId = 0;
  let stopped = false;

  const paint = (time: number) => {
    if (stopped) return;
    const elapsedMs = Math.max(0, time - startedAt);
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.clearRect(0, 0, canvas.width, canvas.height);
    context.setTransform(dpr, 0, 0, dpr, 0, 0);

    let stillRunning = false;
    for (const particle of particles) {
      const visual = thumbnailDustVisualAt(particle, elapsedMs);
      if (visual.opacity <= 0.004) {
        if (elapsedMs < particle.delayMs + particle.durationMs) stillRunning = true;
        continue;
      }
      stillRunning = stillRunning || elapsedMs < particle.delayMs + particle.durationMs;
      const sampleLeft = Math.max(0, particle.sourceLeft);
      const sampleTop = Math.max(0, particle.sourceTop);
      const sampleWidth = Math.max(0.5, Math.min(particle.width, particle.cardWidth - sampleLeft + 0.55));
      const sampleHeight = Math.max(0.5, Math.min(particle.height, particle.cardHeight - sampleTop + 0.55));
      context.save();
      context.globalAlpha = visual.opacity;
      context.translate(
        particle.left + particle.width / 2 + visual.dx,
        particle.top + particle.height / 2 + visual.dy,
      );
      context.rotate(visual.rotate * (Math.PI / 180));
      context.scale(visual.scale, visual.scale);
      context.drawImage(
        source,
        (blurPad + sampleLeft) * sourceScaleX,
        (blurPad + sampleTop) * sourceScaleY,
        sampleWidth * sourceScaleX,
        sampleHeight * sourceScaleY,
        -particle.width / 2,
        -particle.height / 2,
        particle.width,
        particle.height,
      );
      context.restore();
    }

    if (stillRunning) {
      frameId = frame(paint);
    }
  };

  frameId = frame(paint);
  return () => {
    stopped = true;
    if (frameId !== 0) cancelFrame(frameId);
  };
}
