import { useId, type CSSProperties } from "react";

export type RangeSliderMark = {
  value: number;
  label: string;
  shortLabel?: string;
};

type RangeSliderProps = {
  ariaLabel: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  valueText: string;
  marks?: RangeSliderMark[];
  disabled?: boolean;
  className?: string;
  onChange: (value: number) => void;
};

export function RangeSlider({
  ariaLabel,
  value,
  min,
  max,
  step = 1,
  valueText,
  marks = [],
  disabled = false,
  className = "",
  onChange,
}: RangeSliderProps) {
  const id = useId();
  const span = Math.max(1, max - min);
  const progress = Math.min(100, Math.max(0, (value - min) / span * 100));
  const style = { "--range-progress": `${progress}%` } as CSSProperties;

  const hasMarks = marks.length > 0;

  return (
    <div
      className={[
        "range-slider",
        hasMarks ? "has-marks" : "",
        disabled ? "disabled" : "",
        className,
      ].filter(Boolean).join(" ")}
      style={style}
    >
      <div className="range-slider-value">
        <output htmlFor={id}>{valueText}</output>
      </div>
      <div className="range-slider-track">
        <input
          id={id}
          type="range"
          min={min}
          max={max}
          step={step}
          value={value}
          disabled={disabled}
          aria-label={ariaLabel}
          aria-valuetext={valueText}
          onChange={(event) => onChange(Number(event.target.value))}
        />
        {hasMarks && (
          <span className="range-slider-ticks" aria-hidden="true">
            {marks.map((mark) => (
              <i
                key={`${mark.value}-${mark.label}`}
                style={{ left: `${Math.min(100, Math.max(0, (mark.value - min) / span * 100))}%` }}
              />
            ))}
          </span>
        )}
      </div>
      {hasMarks && (
        <span className="range-slider-labels" aria-hidden="true">
          {marks.map((mark) => (
            <small
              key={`${mark.value}-${mark.label}`}
              style={{ left: `${Math.min(100, Math.max(0, (mark.value - min) / span * 100))}%` }}
            >
              {mark.shortLabel ?? mark.label}
            </small>
          ))}
        </span>
      )}
    </div>
  );
}

export type NotchedSliderOption<Value extends string> = {
  value: Value;
  label: string;
  shortLabel?: string;
  description?: string;
};

export function NotchedSlider<Value extends string>({
  ariaLabel,
  value,
  options,
  className = "",
  onChange,
}: {
  ariaLabel: string;
  value: Value;
  options: readonly NotchedSliderOption<Value>[];
  className?: string;
  onChange: (value: Value) => void;
}) {
  const selectedIndex = Math.max(0, options.findIndex((option) => option.value === value));
  const selected = options[selectedIndex];

  return (
    <div className={`notched-slider${className ? ` ${className}` : ""}`}>
      <RangeSlider
        ariaLabel={ariaLabel}
        min={0}
        max={Math.max(0, options.length - 1)}
        value={selectedIndex}
        valueText={selected?.label ?? ""}
        marks={options.map((option, index) => ({
          value: index,
          label: option.label,
          shortLabel: option.shortLabel,
        }))}
        onChange={(index) => {
          const option = options[Math.min(options.length - 1, Math.max(0, Math.round(index)))];
          if (option) onChange(option.value);
        }}
      />
      {selected?.description && (
        <p className="range-slider-description">{selected.description}</p>
      )}
    </div>
  );
}
