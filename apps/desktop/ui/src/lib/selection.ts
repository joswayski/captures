export interface SelectionPoint {
  x: number;
  y: number;
}

export interface SelectionRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type SelectionDragMode = "create" | "move" | "nw" | "ne" | "sw" | "se";

export function frontToBackWindows<T extends { z_order: number }>(windows: readonly T[]): T[] {
  return windows
    .map((window, index) => ({ index, window }))
    .sort((left, right) => (
      right.window.z_order - left.window.z_order
      || left.index - right.index
    ))
    .map(({ window }) => window);
}

export function selectionRect(start: SelectionPoint, end: SelectionPoint): SelectionRect {
  return {
    x: Math.min(start.x, end.x),
    y: Math.min(start.y, end.y),
    width: Math.abs(end.x - start.x),
    height: Math.abs(end.y - start.y),
  };
}

export function isCapturableSelection(
  rect: SelectionRect | null,
): rect is SelectionRect {
  return rect !== null && rect.width >= 2 && rect.height >= 2;
}

export function roundedRectPath(rect: SelectionRect, cornerRadius: number): string {
  const left = rect.x;
  const top = rect.y;
  const right = rect.x + rect.width;
  const bottom = rect.y + rect.height;
  const radius = Math.max(
    0,
    Math.min(cornerRadius, rect.width / 2, rect.height / 2),
  );
  if (radius === 0) {
    return `M${left} ${top}H${right}V${bottom}H${left}Z`;
  }
  return [
    `M${left + radius} ${top}`,
    `H${right - radius}`,
    `A${radius} ${radius} 0 0 1 ${right} ${top + radius}`,
    `V${bottom - radius}`,
    `A${radius} ${radius} 0 0 1 ${right - radius} ${bottom}`,
    `H${left + radius}`,
    `A${radius} ${radius} 0 0 1 ${left} ${bottom - radius}`,
    `V${top + radius}`,
    `A${radius} ${radius} 0 0 1 ${left + radius} ${top}`,
    "Z",
  ].join("");
}

/**
 * CSS clip-path for a full-surface shade with a rectangular hole.
 *
 * Uses the same CSS pixel space as the selection marquee (`left`/`top`/`width`/
 * `height` on a positioned box). Prefer this over an SVG viewBox cutout when
 * the hole is square: on Windows, theoretical display DIPs can disagree with
 * the live WebView client size, which misaligned SVG path units against the
 * marquee.
 *
 * The hole ring must close back to its start. CSS `polygon()` auto-closes the
 * whole path to the first point; without an explicit hole close, that edge
 * runs from the hole's last corner to the screen origin and evenodd leaves a
 * bright diagonal "spotlight" from the top-left into the selection.
 */
export function captureDimClipPath(rect: SelectionRect): string {
  const left = Math.max(0, rect.x);
  const top = Math.max(0, rect.y);
  const right = left + Math.max(0, rect.width);
  const bottom = top + Math.max(0, rect.height);
  return [
    "polygon(evenodd,",
    "0% 0%, 100% 0%, 100% 100%, 0% 100%, 0% 0%,",
    `${left}px ${top}px, ${left}px ${bottom}px, ${right}px ${bottom}px, ${right}px ${top}px, ${left}px ${top}px)`,
  ].join(" ");
}

export function dragSelectionRect(
  mode: SelectionDragMode,
  origin: SelectionPoint,
  current: SelectionPoint,
  initial: SelectionRect,
  bounds: { width: number; height: number },
  minimumSize = 16,
): SelectionRect {
  if (mode === "create") return selectionRect(origin, current);

  const dx = current.x - origin.x;
  const dy = current.y - origin.y;
  if (mode === "move") {
    return {
      ...initial,
      x: clamp(initial.x + dx, 0, Math.max(0, bounds.width - initial.width)),
      y: clamp(initial.y + dy, 0, Math.max(0, bounds.height - initial.height)),
    };
  }

  let left = initial.x;
  let top = initial.y;
  let right = initial.x + initial.width;
  let bottom = initial.y + initial.height;
  if (mode.includes("w")) left = clamp(initial.x + dx, 0, right - minimumSize);
  if (mode.includes("e")) right = clamp(initial.x + initial.width + dx, left + minimumSize, bounds.width);
  if (mode.includes("n")) top = clamp(initial.y + dy, 0, bottom - minimumSize);
  if (mode.includes("s")) bottom = clamp(initial.y + initial.height + dy, top + minimumSize, bounds.height);
  return { x: left, y: top, width: right - left, height: bottom - top };
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), Math.max(minimum, maximum));
}
