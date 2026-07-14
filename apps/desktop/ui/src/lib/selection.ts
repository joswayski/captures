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
