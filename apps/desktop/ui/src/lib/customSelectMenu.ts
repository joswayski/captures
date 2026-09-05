export const CUSTOM_SELECT_VIEWPORT_PADDING = 8;
export const CUSTOM_SELECT_MENU_GAP = 6;
export const CUSTOM_SELECT_MAX_MENU_HEIGHT = 240;
export const CUSTOM_SELECT_MAX_MENU_WIDTH = 360;

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
  const measuredHeight = menu.height
    || Math.min(CUSTOM_SELECT_MAX_MENU_HEIGHT, optionCount * 31 + 8);
  const desiredHeight = Math.min(CUSTOM_SELECT_MAX_MENU_HEIGHT, measuredHeight);
  const spaceAbove = Math.max(0, trigger.top - CUSTOM_SELECT_VIEWPORT_PADDING);
  const spaceBelow = Math.max(0, viewport.height - trigger.bottom - CUSTOM_SELECT_VIEWPORT_PADDING);
  const placement = spaceBelow < desiredHeight && spaceAbove > spaceBelow ? "above" : "below";
  const availableHeight = placement === "above" ? spaceAbove : spaceBelow;
  const viewportMaxHeight = Math.max(0, viewport.height - CUSTOM_SELECT_VIEWPORT_PADDING * 2);
  const maxHeight = Math.max(
    1,
    Math.min(
      CUSTOM_SELECT_MAX_MENU_HEIGHT,
      availableHeight || viewportMaxHeight,
      viewportMaxHeight,
    ),
  );
  const menuHeight = Math.min(maxHeight, measuredHeight);

  const maxWidth = Math.min(
    CUSTOM_SELECT_MAX_MENU_WIDTH,
    Math.max(0, viewport.width - CUSTOM_SELECT_VIEWPORT_PADDING * 2),
  );
  const menuWidth = Math.min(Math.max(menu.width, trigger.width), maxWidth || trigger.width);
  let left = trigger.right - menuWidth;
  const minLeft = CUSTOM_SELECT_VIEWPORT_PADDING;
  const maxLeft = viewport.width - CUSTOM_SELECT_VIEWPORT_PADDING - menuWidth;
  left = Math.min(Math.max(minLeft, left), Math.max(minLeft, maxLeft));

  let top = placement === "above"
    ? trigger.top - CUSTOM_SELECT_MENU_GAP - menuHeight
    : trigger.bottom + CUSTOM_SELECT_MENU_GAP;
  const minTop = CUSTOM_SELECT_VIEWPORT_PADDING;
  const maxTop = Math.max(minTop, viewport.height - menuHeight - CUSTOM_SELECT_VIEWPORT_PADDING);
  top = Math.min(Math.max(minTop, top), maxTop);

  return {
    placement,
    maxHeight,
    top,
    left,
    minWidth: Math.min(trigger.width, maxWidth || trigger.width),
  };
}

/** True when `target` is inside a portaled select owned by `container`. */
export function eventTargetBelongsToSelectIn(
  container: Element | null,
  target: EventTarget | null,
): boolean {
  if (!(target instanceof Element) || !container) return false;
  const listbox = target.closest(".custom-select-listbox");
  if (!listbox?.id) return false;
  return Array.from(container.querySelectorAll("[aria-controls]")).some(
    (element) => element.getAttribute("aria-controls") === listbox.id,
  );
}

