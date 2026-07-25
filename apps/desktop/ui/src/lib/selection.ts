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
