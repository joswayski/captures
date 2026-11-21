import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { createPortal } from "react-dom";
import { normalizeHexColor } from "../../../../shared/themes";

type Hsv = { h: number; s: number; v: number };

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

export function hexToHsv(hex: string): Hsv | null {
  const normalized = normalizeHexColor(hex.slice(0, 7));
  if (!normalized) return null;
  const [r, g, b] = [1, 3, 5].map(
    (index) => parseInt(normalized.slice(index, index + 2), 16) / 255,
  );
  const max = Math.max(r, g, b);
  const delta = max - Math.min(r, g, b);
  let h = 0;
  if (delta > 0) {
    if (max === r) h = ((g - b) / delta) % 6;
    else if (max === g) h = (b - r) / delta + 2;
    else h = (r - g) / delta + 4;
  }
  return { h: (h * 60 + 360) % 360, s: max === 0 ? 0 : delta / max, v: max };
}

export function hsvToHex({ h, s, v }: Hsv): string {
  const channel = (n: number) => {
    const k = (n + h / 60) % 6;
    const value = v - v * s * Math.max(0, Math.min(k, 4 - k, 1));
    return Math.round(value * 255)
      .toString(16)
      .padStart(2, "0");
  };
  return `#${channel(5)}${channel(3)}${channel(1)}`;
}

/** Keep the hue and saturation the user chose when the hex alone cannot (grays, black). */
function syncedHsv(current: Hsv, hex: string): Hsv {
  if (hsvToHex(current) === normalizeHexColor(hex.slice(0, 7))) return current;
  const next = hexToHsv(hex) ?? current;
  if (next.v === 0) return { ...current, v: 0 };
  if (next.s === 0) return { ...current, s: 0, v: next.v };
  return next;
}

function useDragArea(onMove: (x: number, y: number) => void) {
  const update = (event: ReactPointerEvent<HTMLDivElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    onMove(
      clamp((event.clientX - bounds.left) / Math.max(1, bounds.width), 0, 1),
      clamp((event.clientY - bounds.top) / Math.max(1, bounds.height), 0, 1),
    );
  };
  return {
    onPointerDown: (event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      event.currentTarget.focus();
      event.currentTarget.setPointerCapture?.(event.pointerId);
      update(event);
    },
    onPointerMove: (event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.currentTarget.hasPointerCapture?.(event.pointerId)) update(event);
    },
  };
}

/** Arrow keys step 1% (or 1°); Shift steps 10. */
function arrowStep(event: ReactKeyboardEvent): { x: number; y: number } | null {
  const step = event.shiftKey ? 10 : 1;
  const steps: Record<string, { x: number; y: number }> = {
    ArrowLeft: { x: -step, y: 0 },
    ArrowRight: { x: step, y: 0 },
    ArrowUp: { x: 0, y: -step },
    ArrowDown: { x: 0, y: step },
  };
  const delta = steps[event.key];
  if (delta) event.preventDefault();
  return delta ?? null;
}

function ColorPickerPanel({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  const [hsv, setHsv] = useState<Hsv>(() => hexToHsv(value) ?? { h: 0, s: 0, v: 0 });
  const synced = syncedHsv(hsv, value);
  if (synced !== hsv) setHsv(synced);
  const hex = hsvToHex(synced);
  const [hexDraft, setHexDraft] = useState<string | null>(null);

  const commit = (next: Hsv) => {
    setHsv(next);
    setHexDraft(null);
    const nextHex = hsvToHex(next);
    if (nextHex !== normalizeHexColor(value.slice(0, 7))) onChange(nextHex);
  };
  const saturationArea = useDragArea((x, y) => commit({ ...synced, s: x, v: 1 - y }));
  const hueArea = useDragArea((x) => commit({ ...synced, h: Math.min(359.9, x * 360) }));
  const commitHexDraft = () => {
    if (hexDraft === null) return;
    const normalized = normalizeHexColor(hexDraft.startsWith("#") ? hexDraft : `#${hexDraft}`);
    setHexDraft(null);
    if (normalized && normalized !== hex) {
      setHsv(syncedHsv(synced, normalized));
      onChange(normalized);
    }
  };

  return (
    <>
      <div
        className="color-picker-saturation"
        role="slider"
        tabIndex={0}
        aria-label={`${label} saturation and brightness`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(synced.s * 100)}
        aria-valuetext={`Saturation ${Math.round(synced.s * 100)}%, brightness ${Math.round(synced.v * 100)}%`}
        style={{ "--picker-hue": `hsl(${synced.h} 100% 50%)` } as CSSProperties}
        {...saturationArea}
        onKeyDown={(event) => {
          const delta = arrowStep(event);
          if (!delta) return;
          commit({
            ...synced,
            s: clamp(synced.s + delta.x / 100, 0, 1),
            v: clamp(synced.v - delta.y / 100, 0, 1),
          });
        }}
      >
        <span
          className="color-picker-thumb"
          style={{ left: `${synced.s * 100}%`, top: `${(1 - synced.v) * 100}%`, background: hex }}
          aria-hidden="true"
        />
      </div>
      <div
        className="color-picker-hue"
        role="slider"
        tabIndex={0}
        aria-label={`${label} hue`}
        aria-valuemin={0}
        aria-valuemax={360}
        aria-valuenow={Math.round(synced.h)}
        aria-valuetext={`${Math.round(synced.h)}°`}
        {...hueArea}
        onKeyDown={(event) => {
          const delta = arrowStep(event);
          if (!delta) return;
          commit({ ...synced, h: (synced.h + delta.x - delta.y + 360) % 360 });
        }}
      >
        <span
          className="color-picker-thumb"
          style={{ left: `${(synced.h / 360) * 100}%`, background: `hsl(${synced.h} 100% 50%)` }}
          aria-hidden="true"
        />
      </div>
      <div className="color-picker-hex-row">
        <span className="color-picker-preview" style={{ background: hex }} aria-hidden="true" />
        <input
          type="text"
          className="color-picker-hex"
          aria-label={`${label} hex value`}
          value={hexDraft ?? hex.toUpperCase()}
          maxLength={7}
          spellCheck={false}
          onChange={(event) => setHexDraft(event.target.value)}
          onBlur={commitHexDraft}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitHexDraft();
            }
          }}
        />
      </div>
    </>
  );
}

/**
 * Swatch button that opens Captures' own color picker: a saturation and
 * brightness field, a hue strip, and a hex field. Replaces the native color
 * input, which on macOS only offers a preset grid.
 */
export function ColorPickerButton({
  label,
  triggerLabel = `${label}: custom color`,
  value,
  onChange,
  className,
  style,
}: {
  /** Color name the picker's controls are labeled with, such as "Shadow color". */
  label: string;
  triggerLabel?: string;
  value: string;
  onChange: (value: string) => void;
  className?: string;
  style?: CSSProperties;
}) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);

  const positionPopover = useCallback(() => {
    const trigger = triggerRef.current?.getBoundingClientRect();
    const popover = popoverRef.current?.getBoundingClientRect();
    if (!trigger) return;
    const width = popover?.width ?? 240;
    const height = popover?.height ?? 260;
    const gap = 8;
    const padding = 8;
    const roomBelow = window.innerHeight - trigger.bottom - padding;
    const openAbove = roomBelow < height && trigger.top - padding > roomBelow;
    const top = openAbove ? trigger.top - height - gap : trigger.bottom + gap;
    setPosition({
      top: clamp(top, padding, Math.max(padding, window.innerHeight - height - padding)),
      left: clamp(
        trigger.left + trigger.width / 2 - width / 2,
        padding,
        Math.max(padding, window.innerWidth - width - padding),
      ),
    });
  }, []);

  useLayoutEffect(() => {
    if (!open) return undefined;
    positionPopover();
    window.addEventListener("resize", positionPopover);
    document.addEventListener("scroll", positionPopover, true);
    return () => {
      window.removeEventListener("resize", positionPopover);
      document.removeEventListener("scroll", positionPopover, true);
    };
  }, [open, positionPopover]);

  useEffect(() => {
    if (!open) return undefined;
    const closeOutside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !popoverRef.current?.contains(target)) {
        setOpen(false);
      }
    };
    // Capture so Escape closes only this picker, not a menu that hosts it.
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      triggerRef.current?.focus();
    };
    document.addEventListener("pointerdown", closeOutside, true);
    window.addEventListener("keydown", closeOnEscape, true);
    return () => {
      document.removeEventListener("pointerdown", closeOutside, true);
      window.removeEventListener("keydown", closeOnEscape, true);
    };
  }, [open]);

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        className={className}
        style={style}
        aria-label={triggerLabel}
        aria-haspopup="dialog"
        aria-expanded={open}
        title={triggerLabel}
        onClick={() => setOpen((current) => !current)}
      />
      {open &&
        createPortal(
          <div
            ref={popoverRef}
            className="color-picker-popover"
            role="dialog"
            aria-label={triggerLabel}
            style={position ?? { visibility: "hidden" }}
          >
            <ColorPickerPanel label={label} value={value} onChange={onChange} />
          </div>,
          document.body,
        )}
    </>
  );
}
