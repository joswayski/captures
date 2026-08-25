import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";

import { formatFileSize } from "./lib/format";
import { RangeSlider } from "./RangeSlider";

const MIN_PNG_PREVIEW_COLORS = 8;
const MAX_PNG_PREVIEW_COLORS = 256;

// Keep the divider handle clear of the Before/After badges at the frame edges.
const MIN_SPLIT_PERCENT = 6;
const MAX_SPLIT_PERCENT = 94;

export type CompressionPreviewProps = {
  open: boolean;
  beforeUrl: string | null;
  afterUrl: string | null;
  beforeBytes: number | null;
  afterBytes: number | null;
  formatLabel: string;
  qualityLabel: string;
  pending: boolean;
  error: string;
  pngColors?: number | null;
  onPngColorsChange?: (value: number) => void;
  onClose: () => void;
};

/**
 * Full-screen before/after export comparison with a draggable split, similar to
 * compresspng.com — “before” is the current canvas, “after” is the compressed encode.
 * The original shows on the left of the divider and the compressed encode on the right.
 */
export function CompressionPreview({
  open,
  beforeUrl,
  afterUrl,
  beforeBytes,
  afterBytes,
  formatLabel,
  qualityLabel,
  pending,
  error,
  pngColors = null,
  onPngColorsChange,
  onClose,
}: CompressionPreviewProps) {
  const titleId = useId();
  // Reset the split when the after image identity changes without a setState-in-effect.
  const splitKey = afterUrl ?? (open ? "open" : "closed");
  const [splitState, setSplitState] = useState({ key: splitKey, split: 50 });
  const split = splitState.key === splitKey ? splitState.split : 50;
  const setSplit = useCallback((value: number) => {
    setSplitState({
      key: splitKey,
      split: Math.min(MAX_SPLIT_PERCENT, Math.max(MIN_SPLIT_PERCENT, value)),
    });
  }, [splitKey]);

  const [frameWidth, setFrameWidth] = useState(0);
  const frameRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);

  useLayoutEffect(() => {
    if (!open) return;
    const frame = frameRef.current;
    if (!frame) return;
    const update = () => setFrameWidth(frame.clientWidth);
    if (typeof ResizeObserver === "undefined") {
      // jsdom / older environments: measure once after layout.
      queueMicrotask(update);
      return;
    }
    const observer = new ResizeObserver(update);
    observer.observe(frame);
    return () => observer.disconnect();
  }, [open, beforeUrl, afterUrl]);

  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  const setSplitFromClientX = useCallback((clientX: number) => {
    const frame = frameRef.current;
    if (!frame) return;
    const bounds = frame.getBoundingClientRect();
    const next = ((clientX - bounds.left) / Math.max(1, bounds.width)) * 100;
    setSplit(next);
  }, [setSplit]);

  useEffect(() => {
    if (!open) return;
    const onMove = (event: PointerEvent) => {
      if (!draggingRef.current) return;
      setSplitFromClientX(event.clientX);
    };
    const onUp = () => {
      draggingRef.current = false;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
  }, [open, setSplitFromClientX]);

  if (!open) return null;

  const savings = beforeBytes !== null
    && afterBytes !== null
    && beforeBytes > 0
    && afterBytes < beforeBytes
    ? Math.round((1 - afterBytes / beforeBytes) * 100)
    : null;

  // The compressed result fills the frame; the original is revealed left of the
  // divider, so the labels match what each side actually shows.
  const baseUrl = afterUrl ?? beforeUrl;
  const showSplit = Boolean(beforeUrl && afterUrl);

  return (
    <div
      className="compression-preview-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className="compression-preview-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <header className="compression-preview-header">
          <div>
            <h2 id={titleId}>Compression preview</h2>
            <p>
              {formatLabel}
              {qualityLabel ? ` · ${qualityLabel}` : ""}
            </p>
          </div>
          <button
            type="button"
            className="compression-preview-close"
            aria-label="Close compression preview"
            onClick={onClose}
          >
            ×
          </button>
        </header>

        {typeof pngColors === "number" && onPngColorsChange && (
          <div className="compression-preview-colors">
            <span>Colors</span>
            <RangeSlider
              ariaLabel="PNG palette colors"
              min={MIN_PNG_PREVIEW_COLORS}
              max={MAX_PNG_PREVIEW_COLORS}
              value={pngColors}
              valueText={`${pngColors} colors`}
              onChange={(value) => onPngColorsChange(
                Math.min(MAX_PNG_PREVIEW_COLORS, Math.max(MIN_PNG_PREVIEW_COLORS, Math.round(value))),
              )}
            />
          </div>
        )}

        <div
          ref={frameRef}
          className="compression-preview-frame"
          data-pending={pending ? "true" : undefined}
        >
          {baseUrl ? (
            <img
              className="compression-preview-image compression-preview-after"
              src={baseUrl}
              alt={afterUrl ? "After compression" : "Before compression"}
              draggable={false}
            />
          ) : (
            <div className="compression-preview-empty">Preparing preview…</div>
          )}
          {showSplit && (
            <div
              className="compression-preview-before-clip"
              style={{ width: `${split}%` }}
            >
              <img
                className="compression-preview-image compression-preview-before"
                src={beforeUrl ?? undefined}
                alt="Before compression"
                draggable={false}
                style={frameWidth > 0 ? { width: frameWidth } : undefined}
              />
            </div>
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
                    draggingRef.current = true;
                    setSplitFromClientX(event.clientX);
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
        </div>

        {error && <p className="compression-preview-error">{error}</p>}
      </div>
    </div>
  );
}
