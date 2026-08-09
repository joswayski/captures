import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

export type SelectOption = {
  value: string;
  label: string;
  description?: string;
  disabled?: boolean;
};

export function CustomSelect({
  value,
  options,
  onChange,
  ariaLabel,
  disabled = false,
  onOpen,
}: {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  ariaLabel: string;
  disabled?: boolean;
  onOpen?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const [menuLayout, setMenuLayout] = useState<{
    placement: "above" | "below";
    maxHeight: number;
  }>({ placement: "below", maxHeight: 240 });
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listboxRef = useRef<HTMLDivElement>(null);
  const listboxId = useId();
  const enabledIndexes = options.flatMap((option, index) => option.disabled ? [] : [index]);
  const selectedIndex = Math.max(0, options.findIndex((option) => option.value === value));
  const selected = options[selectedIndex] ?? options[0];
  const activeOptionId = `${listboxId}-option-${activeIndex}`;

  const openMenu = () => {
    if (disabled) return;
    onOpen?.();
    setActiveIndex(options[selectedIndex]?.disabled ? (enabledIndexes[0] ?? 0) : selectedIndex);
    setOpen(true);
  };
  const closeMenu = () => setOpen(false);
  const choose = (index: number) => {
    const option = options[index];
    if (!option || option.disabled) return;
    onChange(option.value);
    closeMenu();
    requestAnimationFrame(() => triggerRef.current?.focus());
  };
  const moveActive = (direction: 1 | -1) => {
    if (enabledIndexes.length === 0) return;
    const current = enabledIndexes.indexOf(activeIndex);
    const next = current < 0
      ? (direction === 1 ? 0 : enabledIndexes.length - 1)
      : (current + direction + enabledIndexes.length) % enabledIndexes.length;
    setActiveIndex(enabledIndexes[next]);
  };

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) closeMenu();
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    return () => window.removeEventListener("pointerdown", onPointerDown, true);
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current || !listboxRef.current) return;
    const triggerBounds = triggerRef.current.getBoundingClientRect();
    const measuredHeight = listboxRef.current.scrollHeight
      || Math.min(240, options.length * 31 + 8);
    const desiredHeight = Math.min(240, measuredHeight);
    const spaceAbove = Math.max(0, triggerBounds.top - 8);
    const spaceBelow = Math.max(0, window.innerHeight - triggerBounds.bottom - 8);
    const placement = spaceBelow < desiredHeight && spaceAbove > spaceBelow ? "above" : "below";
    const availableHeight = placement === "above" ? spaceAbove : spaceBelow;
    const nextLayout = {
      placement,
      maxHeight: Math.max(72, Math.min(240, availableHeight - 5)),
    } as const;
    setMenuLayout((current) => (
      current.placement === nextLayout.placement && current.maxHeight === nextLayout.maxHeight
        ? current
        : nextLayout
    ));
  }, [open, options.length]);

  return (
    <div
      className={`custom-select${open ? " open" : ""}${open && menuLayout.placement === "above" ? " open-above" : ""}`}
      ref={rootRef}
    >
      <button
        ref={triggerRef}
        type="button"
        className="custom-select-trigger"
        role="combobox"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listboxId : undefined}
        aria-activedescendant={open ? activeOptionId : undefined}
        disabled={disabled}
        onClick={() => open ? closeMenu() : openMenu()}
        onBlur={(event) => {
          if (!rootRef.current?.contains(event.relatedTarget as Node | null)) closeMenu();
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape" && open) {
            event.preventDefault();
            closeMenu();
          } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            if (!open) openMenu();
            else moveActive(event.key === "ArrowDown" ? 1 : -1);
          } else if (event.key === "Home" && open) {
            event.preventDefault();
            setActiveIndex(enabledIndexes[0] ?? 0);
          } else if (event.key === "End" && open) {
            event.preventDefault();
            setActiveIndex(enabledIndexes.at(-1) ?? 0);
          } else if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            if (open) choose(activeIndex);
            else openMenu();
          }
        }}
      >
        <span>{selected?.label ?? value}</span>
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 6 4 4 4-4" /></svg>
      </button>
      {open && (
        <div
          ref={listboxRef}
          id={listboxId}
          className="custom-select-listbox"
          role="listbox"
          aria-label={ariaLabel}
          style={{ maxHeight: menuLayout.maxHeight }}
        >
          {options.map((option, index) => (
            <button
              key={`${option.value}-${index}`}
              id={`${listboxId}-option-${index}`}
              type="button"
              role="option"
              aria-selected={option.value === value}
              disabled={option.disabled}
              className={activeIndex === index ? "active" : ""}
              onPointerEnter={() => {
                if (!option.disabled) setActiveIndex(index);
              }}
              onClick={() => choose(index)}
            >
              <span className="custom-select-option-copy">
                <span>{option.label}</span>
                {option.description && <small>{option.description}</small>}
              </span>
              {option.value === value && <span aria-hidden="true">✓</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
