import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

export type SelectOption = {
  value: string;
  label: string;
  description?: string;
  disabled?: boolean;
};

const VIEWPORT_PADDING = 8;
const MENU_GAP = 6;
const MAX_MENU_HEIGHT = 240;
const MAX_MENU_WIDTH = 360;

export type CustomSelectMenuLayout = {
  placement: "above" | "below";
  maxHeight: number;
  top: number;
  left: number;
  minWidth: number;
};

/** Viewport box used by `placeCustomSelectMenu`. */
export type CustomSelectMenuBox = {
  top: number;
  left: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
};

/**
 * Place a select menu next to its trigger without crossing the window edge.
 * Prefer the trigger’s right edge (the in-flow look), then shift so the card
 * stays fully on-screen — including when the trigger sits in a clipped editor.
 */
export function placeCustomSelectMenu(
  trigger: CustomSelectMenuBox,
  menu: { width: number; height: number },
  viewport: { width: number; height: number },
  optionCount: number,
): CustomSelectMenuLayout {
  const measuredHeight = menu.height || Math.min(MAX_MENU_HEIGHT, optionCount * 31 + 8);
  const desiredHeight = Math.min(MAX_MENU_HEIGHT, measuredHeight);
  const spaceAbove = Math.max(0, trigger.top - VIEWPORT_PADDING);
  const spaceBelow = Math.max(0, viewport.height - trigger.bottom - VIEWPORT_PADDING);
  const placement = spaceBelow < desiredHeight && spaceAbove > spaceBelow ? "above" : "below";
  const availableHeight = placement === "above" ? spaceAbove : spaceBelow;
  const viewportMaxHeight = Math.max(0, viewport.height - VIEWPORT_PADDING * 2);
  const maxHeight = Math.max(
    1,
    Math.min(MAX_MENU_HEIGHT, availableHeight || viewportMaxHeight, viewportMaxHeight),
  );
  const menuHeight = Math.min(maxHeight, measuredHeight);

  const maxWidth = Math.min(MAX_MENU_WIDTH, Math.max(0, viewport.width - VIEWPORT_PADDING * 2));
  const menuWidth = Math.min(Math.max(menu.width, trigger.width), maxWidth || trigger.width);
  let left = trigger.right - menuWidth;
  const minLeft = VIEWPORT_PADDING;
  const maxLeft = viewport.width - VIEWPORT_PADDING - menuWidth;
  left = Math.min(Math.max(minLeft, left), Math.max(minLeft, maxLeft));

  let top = placement === "above"
    ? trigger.top - MENU_GAP - menuHeight
    : trigger.bottom + MENU_GAP;
  const minTop = VIEWPORT_PADDING;
  const maxTop = Math.max(minTop, viewport.height - menuHeight - VIEWPORT_PADDING);
  top = Math.min(Math.max(minTop, top), maxTop);

  return {
    placement,
    maxHeight,
    top,
    left,
    minWidth: Math.min(trigger.width, maxWidth || trigger.width),
  };
}

function menuContainsTarget(
  root: HTMLElement | null,
  listbox: HTMLElement | null,
  node: Node | null,
) {
  return Boolean(node && (root?.contains(node) || listbox?.contains(node)));
}

function isGlassSelect(root: HTMLElement | null) {
  return Boolean(root?.closest(".on-media, .recording-selector-panel, .recording-hud"));
}

export function CustomSelect({
  value,
  options,
  onChange,
  ariaLabel,
  className,
  triggerLabel,
  disabled = false,
  onOpen,
}: {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  ariaLabel: string;
  className?: string;
  triggerLabel?: string;
  disabled?: boolean;
  onOpen?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const [menuLayout, setMenuLayout] = useState<CustomSelectMenuLayout>({
    placement: "below",
    maxHeight: MAX_MENU_HEIGHT,
    top: 0,
    left: 0,
    minWidth: 0,
  });
  const [glass, setGlass] = useState(false);
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
    setGlass(isGlassSelect(rootRef.current));
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
      if (!menuContainsTarget(rootRef.current, listboxRef.current, event.target as Node)) {
        closeMenu();
      }
    };
    window.addEventListener("pointerdown", onPointerDown, true);
    return () => window.removeEventListener("pointerdown", onPointerDown, true);
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current || !listboxRef.current) return undefined;
    const place = () => {
      const trigger = triggerRef.current;
      const listbox = listboxRef.current;
      if (!trigger || !listbox) return;
      const triggerBounds = trigger.getBoundingClientRect();
      const nextLayout = placeCustomSelectMenu(
        triggerBounds,
        {
          width: Math.max(listbox.scrollWidth, listbox.offsetWidth),
          height: listbox.scrollHeight,
        },
        { width: window.innerWidth, height: window.innerHeight },
        options.length,
      );
      setMenuLayout((current) => (
        current.placement === nextLayout.placement
          && current.maxHeight === nextLayout.maxHeight
          && current.top === nextLayout.top
          && current.left === nextLayout.left
          && current.minWidth === nextLayout.minWidth
          ? current
          : nextLayout
      ));
    };
    place();
    window.addEventListener("resize", place);
    document.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      document.removeEventListener("scroll", place, true);
    };
  }, [open, options.length]);

  const listbox = open && (
    <div
      ref={listboxRef}
      id={listboxId}
      className={[
        "custom-select-listbox",
        className?.includes("filename-format-select") ? "filename-format-select-listbox" : "",
        glass ? "custom-select-listbox-glass" : "",
      ].filter(Boolean).join(" ")}
      role="listbox"
      aria-label={ariaLabel}
      style={{
        position: "fixed",
        top: menuLayout.top,
        left: menuLayout.left,
        minWidth: menuLayout.minWidth,
        maxHeight: menuLayout.maxHeight,
        maxWidth: MAX_MENU_WIDTH,
      }}
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
  );

  return (
    <div
      className={[
        "custom-select",
        open ? "open" : "",
        open && menuLayout.placement === "above" ? "open-above" : "",
        className,
      ].filter(Boolean).join(" ")}
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
          if (!menuContainsTarget(
            rootRef.current,
            listboxRef.current,
            event.relatedTarget as Node | null,
          )) closeMenu();
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
        <span>{triggerLabel ?? selected?.label ?? value}</span>
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 6 4 4 4-4" /></svg>
      </button>
      {listbox && createPortal(listbox, document.body)}
    </div>
  );
}
