import { useCallback, useLayoutEffect, useRef, useState } from "react";

import { formatFileSize } from "./lib/format";

// Keep the divider handle clear of the Before/After badges at the frame edges.
const MIN_SPLIT_PERCENT = 6;
const MAX_SPLIT_PERCENT = 94;

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
  className = "",
}: CompressionPreviewProps) {
  // Reset the split when the after image identity changes without a setState-in-effect.
  const splitKey = afterUrl ?? (liveBefore ? "live" : "empty");
  const [splitState, setSplitState] = useState({ key: splitKey, split: 50 });
  const split = splitState.key === splitKey ? splitState.split : 50;
  const setSplit = useCallback((value: number) => {
    setSplitState({
      key: splitKey,
      split: Math.min(MAX_SPLIT_PERCENT, Math.max(MIN_SPLIT_PERCENT, value)),
    });
  }, [splitKey]);

  const [frameSize, setFrameSize] = useState({ width: 0, height: 0 });
  const frameRef = useRef<HTMLDivElement>(null);
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

  const setSplitFromClientX = useCallback((clientX: number) => {
    const frame = frameRef.current;
    if (!frame) return;
    const bounds = frame.getBoundingClientRect();
    const next = ((clientX - bounds.left) / Math.max(1, bounds.width)) * 100;
    setSplit(next);
  }, [setSplit]);

  const beginSplitDrag = useCallback((clientX: number) => {
    draggingRef.current = true;
    setSplitFromClientX(clientX);
  }, [setSplitFromClientX]);

  const savings = beforeBytes !== null
    && afterBytes !== null
    && beforeBytes > 0
    && afterBytes < beforeBytes
    ? Math.round((1 - afterBytes / beforeBytes) * 100)
    : null;

  const showAfter = Boolean(afterUrl);
  const showBeforeImage = Boolean(beforeUrl) && !liveBefore;
  const showSplit = showAfter && (liveBefore || showBeforeImage);
  const { width: frameWidth, height: frameHeight } = frameSize;

  return (
    <div
      ref={frameRef}
      className={[
        "compression-preview-frame",
        liveBefore ? "is-live" : "",
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
          ) : (
            <div className="compression-preview-empty">Preparing preview…</div>
          )}
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
              onPointerDown={(event) => {
                event.preventDefault();
                event.stopPropagation();
                event.currentTarget.setPointerCapture(event.pointerId);
                beginSplitDrag(event.clientX);
              }}
              onPointerMove={(event) => {
                if (!draggingRef.current) return;
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
            onChange={(event) => setSplit(Number(event.target.value))}
          />
        </>
      )}
      <span className="compression-preview-badge is-before" aria-live="polite">
        Before
        {beforeBytes !== null && ` · ${formatFileSize(beforeBytes)}`}
      </span>
      <span className="compression-preview-badge is-after" aria-live="polite">
        After
        {pending
          ? " · Encoding…"
          : afterBytes !== null && (
            <>
              {` · ${formatFileSize(afterBytes)}`}
              {savings !== null && (
                <span className="compression-preview-savings"> · {savings}% smaller</span>
              )}
            </>
          )}
      </span>
      {error && <p className="compression-preview-error">{error}</p>}
    </div>
  );
}
