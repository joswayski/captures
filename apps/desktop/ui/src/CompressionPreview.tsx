import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";

import { formatFileSize } from "./lib/format";

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
  onClose: () => void;
};

/**
 * Full-screen before/after export comparison with a draggable split, similar to
 * compresspng.com — “before” is the current canvas, “after” is the compressed encode.
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
  onClose,
}: CompressionPreviewProps) {
  const titleId = useId();
  const [split, setSplit] = useState(50);
  const [frameWidth, setFrameWidth] = useState(0);
  const frameRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);

  useEffect(() => {
    if (!open) return;
    setSplit(50);
  }, [open, afterUrl]);

  useLayoutEffect(() => {
    if (!open) return;
    const frame = frameRef.current;
    if (!frame) return;
    const update = () => setFrameWidth(frame.clientWidth);
    update();
    if (typeof ResizeObserver === "undefined") return;
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
    setSplit(Math.min(100, Math.max(0, next)));
  }, []);

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
              {savings !== null ? (
                <>
                  {" · "}
                  <span className="compression-preview-savings">−{savings}%</span>
                </>
              ) : null}
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

        <div className="compression-preview-stats" aria-live="polite">
          <span>
            Before
            {" "}
            <strong>
              {beforeBytes === null ? "—" : formatFileSize(beforeBytes)}
            </strong>
          </span>
          <span>
            After
            {" "}
            <strong className={savings !== null ? "is-smaller" : undefined}>
              {pending
                ? "Encoding…"
                : afterBytes === null
                  ? "—"
                  : formatFileSize(afterBytes)}
            </strong>
          </span>
        </div>

        <div
          ref={frameRef}
          className="compression-preview-frame"
          data-pending={pending ? "true" : undefined}
        >
          {beforeUrl ? (
            <img
              className="compression-preview-image compression-preview-before"
              src={beforeUrl}
              alt="Before compression"
              draggable={false}
            />
          ) : (
            <div className="compression-preview-empty">Preparing original…</div>
          )}
          {afterUrl && (
            <div
              className="compression-preview-after-clip"
              style={{ width: `${split}%` }}
            >
              <img
                className="compression-preview-image compression-preview-after"
                src={afterUrl}
                alt="After compression"
                draggable={false}
                style={frameWidth > 0 ? { width: frameWidth } : undefined}
              />
            </div>
          )}
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
            min={0}
            max={100}
            step={0.1}
            value={split}
            aria-label="Before and after comparison"
            onChange={(event) => setSplit(Number(event.target.value))}
          />
          <span className="compression-preview-badge is-before">Before</span>
          <span className="compression-preview-badge is-after">After</span>
        </div>

        {error && <p className="compression-preview-error">{error}</p>}
        <p className="compression-preview-hint">
          Drag the handle to compare. This is the same encode used when you Save — it does not write a file.
        </p>
      </div>
    </div>
  );
}
