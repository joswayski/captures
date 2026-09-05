import { useCallback, useLayoutEffect, useRef, useState } from "react";

import { formatFileSize } from "./lib/format";

// Keep the divider handle clear of the Before/After badges at the frame edges.
const MIN_SPLIT_PERCENT = 6;
const MAX_SPLIT_PERCENT = 94;
const AFTER_HINT_PAD = 8;
const AFTER_HINT_FALLBACK_WIDTH = 220;
const AFTER_HINT_FALLBACK_HEIGHT = 40;

/** Keep the After-side cursor hint fully inside the comparison frame. */
function compressionAfterHintPosition(
  localX: number,
  localY: number,
  frameWidth: number,
  frameHeight: number,
  hintWidth: number,
  hintHeight: number,
): { x: number; y: number } {
  const width = Math.min(hintWidth, Math.max(1, frameWidth - AFTER_HINT_PAD * 2));
  const height = Math.min(hintHeight, Math.max(1, frameHeight - AFTER_HINT_PAD * 2));
  let x = localX + 12;
  let y = localY + 14;
  if (x + width + AFTER_HINT_PAD > frameWidth) x = localX - width - AFTER_HINT_PAD;
  if (y + height + AFTER_HINT_PAD > frameHeight) y = localY - height - AFTER_HINT_PAD;
  return {
    x: Math.min(frameWidth - width - AFTER_HINT_PAD, Math.max(AFTER_HINT_PAD, x)),
    y: Math.min(frameHeight - height - AFTER_HINT_PAD, Math.max(AFTER_HINT_PAD, y)),
  };
}

export type CompressionPreviewProps = {
  beforeUrl: string | null;
  afterUrl: string | null;
  beforeBytes: number | null;
  afterBytes: number | null;
  pending: boolean;
  error?: string;
  /**
   * Left side is live editor content (no before image). The compressed encode
   * is clipped to the right of the divider so it sits on the canvas or preview.
   */
  liveBefore?: boolean;
  /** Fade the comparison away so the live canvas can be edited underneath. */
  suppressed?: boolean;
  /**
   * Cursor-following hint while the pointer is on the compressed (after) side.
   * Drawing still goes to the original; this side is a frozen encode.
   */
  afterHint?: string;
  /** Hide the comparison overlay without changing save quality. */
  onDismiss?: () => void;
  /** Starting split when this overlay mounts. */
  initialSplit?: number;
  onSplitChange?: (split: number) => void;
  /**
   * When false, the hidden full-width range ignores pointers so drawing can
   * pass through. The circular divider handle stays draggable.
   */
  splitDragEnabled?: boolean;
  className?: string;
};

function overlayBoxStyle(
  frameWidth: number,
  frameHeight: number,
  split: number,
  origin: "before" | "after",
): { width: number; height: number; left?: string } | undefined {
  if (frameWidth <= 0 || frameHeight <= 0) return undefined;
  return {
    width: frameWidth,
    height: frameHeight,
    ...(origin === "after" ? { left: `${-(split / 100) * frameWidth}px` } : {}),
  };
}

/**
 * Paint the compressed encode into a canvas whose backing store is the file's
 * native pixels and whose CSS box matches the live editor canvas. Scaling an
 * `<img>` uses a different interpolator than `<canvas>`, which makes text
 * blobs look like a different typeface at the before/after split.
 */
function LiveAfterCanvas({
  url,
  frameWidth,
  frameHeight,
  split,
}: {
  url: string;
  frameWidth: number;
  frameHeight: number;
  split: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const image = new Image();
    let cancelled = false;
    image.onload = () => {
      if (cancelled) return;
      const width = Math.max(1, image.naturalWidth || image.width);
      const height = Math.max(1, image.naturalHeight || image.height);
      if (canvas.width !== width) canvas.width = width;
      if (canvas.height !== height) canvas.height = height;
      const context = canvas.getContext("2d");
      if (!context) return;
      context.imageSmoothingEnabled = false;
      context.clearRect(0, 0, width, height);
      context.drawImage(image, 0, 0, width, height);
    };
    image.src = url;
    return () => {
      cancelled = true;
    };
  }, [url]);

  return (
    <canvas
      ref={canvasRef}
      className="compression-preview-image compression-preview-after"
      role="img"
      aria-label="After compression"
      style={overlayBoxStyle(frameWidth, frameHeight, split, "after")}
    />
  );
}

/**
 * Draggable before/after split for export comparison.
 * “Before” is the current canvas or source frame; “after” is the compressed encode.
 */
export function CompressionPreview({
  beforeUrl,
  afterUrl,
  beforeBytes,
  afterBytes,
  pending,
  error = "",
  liveBefore = false,
  suppressed = false,
  afterHint = "",
  onDismiss,
  initialSplit = 50,
  onSplitChange,
  splitDragEnabled = true,
  className = "",
}: CompressionPreviewProps) {
  const [split, setSplitState] = useState(() => (
    Math.min(MAX_SPLIT_PERCENT, Math.max(MIN_SPLIT_PERCENT, initialSplit))
  ));
  const setSplit = useCallback((value: number) => {
    const next = Math.min(MAX_SPLIT_PERCENT, Math.max(MIN_SPLIT_PERCENT, value));
    setSplitState(next);
    onSplitChange?.(next);
  }, [onSplitChange]);

  const [frameSize, setFrameSize] = useState({ width: 0, height: 0 });
  const [afterHintPos, setAfterHintPos] = useState<{ x: number; y: number } | null>(null);
  const frameRef = useRef<HTMLDivElement>(null);
  const afterHintRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);

  useLayoutEffect(() => {
    const frame = frameRef.current;
    if (!frame) return;
    const update = () => setFrameSize({
      width: frame.clientWidth,
      height: frame.clientHeight,
    });
    if (typeof ResizeObserver === "undefined") {
      queueMicrotask(update);
      return;
    }
    const observer = new ResizeObserver(update);
    observer.observe(frame);
    return () => observer.disconnect();
  }, [beforeUrl, afterUrl, liveBefore]);

  useLayoutEffect(() => {
    if (!afterHint || suppressed) return;
    const updateHint = (clientX: number, clientY: number) => {
      const frame = frameRef.current;
      if (!frame) return;
      const bounds = frame.getBoundingClientRect();
      if (
        clientX < bounds.left
        || clientX > bounds.right
        || clientY < bounds.top
        || clientY > bounds.bottom
      ) {
        setAfterHintPos(null);
        return;
      }
      const percent = ((clientX - bounds.left) / Math.max(1, bounds.width)) * 100;
      if (percent <= split) {
        setAfterHintPos(null);
        return;
      }
      const hint = afterHintRef.current;
      setAfterHintPos(compressionAfterHintPosition(
        clientX - bounds.left,
        clientY - bounds.top,
        bounds.width,
        bounds.height,
        hint?.offsetWidth || AFTER_HINT_FALLBACK_WIDTH,
        hint?.offsetHeight || AFTER_HINT_FALLBACK_HEIGHT,
      ));
    };
    const onPointerMove = (event: PointerEvent) => {
      updateHint(event.clientX, event.clientY);
    };
    const onPointerLeave = () => setAfterHintPos(null);
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("blur", onPointerLeave);
    return () => {
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("blur", onPointerLeave);
    };
  }, [afterHint, split, suppressed]);

  const processing = pending && !suppressed;
  // Keep the circular handle draggable even while a drawing tool is selected.
  // The hidden range still yields so strokes can start on the canvas.
  const canDragHandle = !processing;
  const canDragRange = splitDragEnabled && canDragHandle;

  const setSplitFromClientX = useCallback((clientX: number) => {
    const frame = frameRef.current;
    if (!frame) return;
    const bounds = frame.getBoundingClientRect();
    const next = ((clientX - bounds.left) / Math.max(1, bounds.width)) * 100;
    setSplit(next);
  }, [setSplit]);

  const beginSplitDrag = useCallback((clientX: number) => {
    if (!canDragHandle) return;
    draggingRef.current = true;
    setSplitFromClientX(clientX);
  }, [canDragHandle, setSplitFromClientX]);

  const savings = beforeBytes !== null
    && afterBytes !== null
    && beforeBytes > 0
    && afterBytes < beforeBytes
    ? Math.round((1 - afterBytes / beforeBytes) * 100)
    : null;

  const showAfter = Boolean(afterUrl);
  const showBeforeImage = Boolean(beforeUrl) && !liveBefore;
  const showSplit = showAfter && (liveBefore || showBeforeImage);
  const waiting = !showAfter && !showBeforeImage;
  const embedded = liveBefore
    || className.includes("is-embed")
    || className.includes("is-cover")
    || className.includes("is-live");
  const { width: frameWidth, height: frameHeight } = frameSize;
  const showAfterHint = Boolean(afterHint && afterHintPos && !suppressed && showSplit);

  return (
    <div
      ref={frameRef}
      className={[
        "compression-preview-frame",
        liveBefore ? "is-live" : "",
        waiting ? "is-waiting" : "",
        suppressed ? "is-suppressed" : "",
        canDragRange ? "" : "is-draw-locked",
        processing ? "is-processing" : "",
        className,
      ].filter(Boolean).join(" ")}
      data-pending={pending ? "true" : undefined}
      role="group"
      aria-label="Compression comparison"
    >
      {liveBefore ? (
        showAfter && afterUrl && (
          <div
            className="compression-preview-after-clip"
            style={{ left: `${split}%` }}
          >
            <LiveAfterCanvas
              url={afterUrl}
              frameWidth={frameWidth}
              frameHeight={frameHeight}
              split={split}
            />
          </div>
        )
      ) : (
        <>
          {showAfter ? (
            <img
              className="compression-preview-image compression-preview-after"
              src={afterUrl ?? undefined}
              alt="After compression"
              draggable={false}
            />
          ) : beforeUrl ? (
            <img
              className="compression-preview-image compression-preview-before"
              src={beforeUrl}
              alt="Before compression"
              draggable={false}
            />
          ) : waiting && !embedded ? (
            <div className="compression-preview-empty">Preparing preview…</div>
          ) : null}
          {showBeforeImage && (
            <div
              className="compression-preview-before-clip"
              style={{ width: `${split}%` }}
            >
              <img
                className="compression-preview-image compression-preview-before"
                src={beforeUrl ?? undefined}
                alt="Before compression"
                draggable={false}
                style={overlayBoxStyle(frameWidth, frameHeight, split, "before")}
              />
            </div>
          )}
        </>
      )}
      <div className="compression-preview-veil" aria-hidden="true" />
      {showSplit && (
        <>
          <div
            className="compression-preview-divider"
            style={{ left: `${split}%` }}
          >
            <button
              type="button"
              className="compression-preview-handle"
              aria-label="Drag to compare before and after"
              disabled={!canDragHandle}
              onPointerDown={(event) => {
                if (!canDragHandle) return;
                event.preventDefault();
                event.stopPropagation();
                event.currentTarget.setPointerCapture(event.pointerId);
                beginSplitDrag(event.clientX);
              }}
              onPointerMove={(event) => {
                if (!canDragHandle || !draggingRef.current) return;
                setSplitFromClientX(event.clientX);
              }}
              onPointerUp={() => {
                draggingRef.current = false;
              }}
              onPointerCancel={() => {
                draggingRef.current = false;
              }}
            >
              ‹ ›
            </button>
          </div>
          <input
            className="compression-preview-range"
            type="range"
            min={MIN_SPLIT_PERCENT}
            max={MAX_SPLIT_PERCENT}
            step={0.1}
            value={split}
            aria-label="Before and after comparison"
            disabled={!canDragRange}
            onChange={(event) => {
              if (!canDragRange) return;
              setSplit(Number(event.target.value));
            }}
          />
        </>
      )}
      <span className="compression-preview-badge is-before" aria-live="polite">
        Before
        {beforeBytes !== null && ` · ${formatFileSize(beforeBytes)}`}
      </span>
      <span className="compression-preview-badge is-after" aria-live="polite">
        After
        {processing
          ? " · Processing…"
          : afterBytes !== null && (
            <>
              {` · ${formatFileSize(afterBytes)}`}
              {savings !== null && (
                <span className="compression-preview-savings"> · {savings}% smaller</span>
              )}
            </>
          )}
      </span>
      {onDismiss && (
        <button
          type="button"
          className="compression-preview-dismiss"
          aria-label="Hide compression comparison"
          onClick={(event) => {
            event.stopPropagation();
            onDismiss();
          }}
        >
          Hide
        </button>
      )}
      {showAfterHint && afterHintPos && (
        <div
          ref={afterHintRef}
          className="compression-preview-after-hint"
          role="tooltip"
          style={{ left: afterHintPos.x, top: afterHintPos.y }}
        >
          {afterHint}
        </div>
      )}
      {processing && (
        <div
          className="compression-preview-processing"
          role="status"
          aria-label="Compression processing"
          aria-live="polite"
        >
          <span className="compression-preview-spinner" aria-hidden="true" />
          Processing
        </div>
      )}
      {error && <p className="compression-preview-error">{error}</p>}
    </div>
  );
}
