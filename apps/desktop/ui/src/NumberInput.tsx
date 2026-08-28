import {
  useCallback,
  useId,
  type ChangeEvent,
  type InputHTMLAttributes,
  type KeyboardEvent,
} from "react";

export type NumberInputProps = {
  value: number | string;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  readOnly?: boolean;
  /** Hide custom steppers (still hides native spinners). */
  hideSteppers?: boolean;
  /** Smaller hit targets for dense chrome like canvas W/H. */
  compact?: boolean;
  ariaLabel?: string;
  className?: string;
  inputMode?: InputHTMLAttributes<HTMLInputElement>["inputMode"];
  title?: string;
  /** Numeric controlled fields (canvas size, crop, font size, …). */
  onChange?: (value: number) => void;
  /**
   * Freeform string fields (e.g. maximum file size while typing "1.").
   * When set, typing calls this; steppers still adjust by `step` and write a string.
   */
  onTextChange?: (value: string) => void;
};

function decimalPlaces(step: number): number {
  const text = String(step);
  const dot = text.indexOf(".");
  return dot === -1 ? 0 : text.length - dot - 1;
}

function parseCurrent(value: number | string): number {
  if (typeof value === "number") {
    return Number.isFinite(value) ? value : Number.NaN;
  }
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : Number.NaN;
}

function formatStepped(value: number, step: number): string {
  const places = decimalPlaces(step);
  if (places === 0) return String(Math.round(value));
  // Round to step precision, then drop trailing zeros (1.50 → 1.5).
  return String(Number(value.toFixed(places)));
}

function clamp(value: number, min?: number, max?: number): number {
  let next = value;
  if (min !== undefined && Number.isFinite(min)) next = Math.max(min, next);
  if (max !== undefined && Number.isFinite(max)) next = Math.min(max, next);
  return next;
}

function stepFrom(
  value: number | string,
  step: number,
  direction: 1 | -1,
  min?: number,
  max?: number,
): number {
  const current = parseCurrent(value);
  const base = Number.isFinite(current)
    ? current
    : (min !== undefined && Number.isFinite(min) ? min : 0);
  const places = decimalPlaces(step);
  const raw = base + step * direction;
  const rounded = places === 0 ? Math.round(raw) : Number(raw.toFixed(places));
  return clamp(rounded, min, max);
}

/**
 * Number field with large custom increment/decrement buttons.
 * Native browser spinners are tiny on macOS WebKit; these replace them.
 */
export function NumberInput({
  value,
  min,
  max,
  step = 1,
  disabled = false,
  readOnly = false,
  hideSteppers = false,
  compact = false,
  ariaLabel,
  className = "",
  inputMode,
  title,
  onChange,
  onTextChange,
}: NumberInputProps) {
  const inputId = useId();
  const showSteppers = !hideSteppers && !readOnly && !disabled;
  const canEdit = !disabled && !readOnly;

  const emitValue = useCallback((next: number) => {
    if (onTextChange) {
      onTextChange(formatStepped(next, step));
      return;
    }
    onChange?.(next);
  }, [onChange, onTextChange, step]);

  const handleInput = useCallback((event: ChangeEvent<HTMLInputElement>) => {
    if (!canEdit) return;
    if (onTextChange) {
      onTextChange(event.target.value);
      return;
    }
    onChange?.(Number(event.target.value));
  }, [canEdit, onChange, onTextChange]);

  const nudge = useCallback((direction: 1 | -1) => {
    if (!canEdit) return;
    emitValue(stepFrom(value, step, direction, min, max));
  }, [canEdit, emitValue, max, min, step, value]);

  const handleKeyDown = useCallback((event: KeyboardEvent<HTMLInputElement>) => {
    if (!canEdit || !showSteppers) return;
    if (event.key === "ArrowUp") {
      event.preventDefault();
      nudge(1);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      nudge(-1);
    }
  }, [canEdit, nudge, showSteppers]);

  const current = parseCurrent(value);
  const atMin = min !== undefined && Number.isFinite(current) && current <= min;
  const atMax = max !== undefined && Number.isFinite(current) && current >= max;

  return (
    <div
      className={[
        "number-input",
        compact ? "is-compact" : "",
        showSteppers ? "has-steppers" : "",
        disabled ? "is-disabled" : "",
        readOnly ? "is-readonly" : "",
        className,
      ].filter(Boolean).join(" ")}
    >
      <input
        id={inputId}
        type="number"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        readOnly={readOnly}
        aria-label={ariaLabel}
        title={title}
        inputMode={inputMode}
        onChange={handleInput}
        onKeyDown={handleKeyDown}
      />
      {showSteppers && (
        <div className="number-input-steppers" aria-hidden={false}>
          <button
            type="button"
            tabIndex={-1}
            className="number-input-step number-input-step-up"
            aria-label={ariaLabel ? `Increase ${ariaLabel}` : "Increase"}
            disabled={disabled || atMax}
            onClick={() => nudge(1)}
          >
            <svg viewBox="0 0 12 12" aria-hidden="true">
              <path d="M3 7.5 6 4.5 9 7.5" />
            </svg>
          </button>
          <button
            type="button"
            tabIndex={-1}
            className="number-input-step number-input-step-down"
            aria-label={ariaLabel ? `Decrease ${ariaLabel}` : "Decrease"}
            disabled={disabled || atMin}
            onClick={() => nudge(-1)}
          >
            <svg viewBox="0 0 12 12" aria-hidden="true">
              <path d="M3 4.5 6 7.5 9 4.5" />
            </svg>
          </button>
        </div>
      )}
    </div>
  );
}
