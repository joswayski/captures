export type EditorCropHandle = "move" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "nw";

type Rect = { x: number; y: number; width: number; height: number };
type Point = { x: number; y: number };

function clamp(value: number, minimum: number, maximum: number): number {
  if (!Number.isFinite(value)) return minimum;
  return Math.min(Math.max(value, minimum), Math.max(minimum, maximum));
}

export function editorCropAfterDrag(
  initial: Rect,
  handle: EditorCropHandle,
  delta: Point,
  bounds: { width: number; height: number },
  lockAspect: boolean,
): Rect {
  if (handle === "move") {
    return {
      ...initial,
      x: clamp(initial.x + delta.x, 0, bounds.width - initial.width),
      y: clamp(initial.y + delta.y, 0, bounds.height - initial.height),
    };
  }

  const west = handle.includes("w");
  const east = handle.includes("e");
  const north = handle.includes("n");
  const south = handle.includes("s");
  let left = initial.x + (west ? delta.x : 0);
  let right = initial.x + initial.width + (east ? delta.x : 0);
  let top = initial.y + (north ? delta.y : 0);
  let bottom = initial.y + initial.height + (south ? delta.y : 0);

  if (!lockAspect) {
    if (west) left = clamp(left, 0, right - 2);
    if (east) right = clamp(right, left + 2, bounds.width);
    if (north) top = clamp(top, 0, bottom - 2);
    if (south) bottom = clamp(bottom, top + 2, bounds.height);
    return {
      x: Math.round(left),
      y: Math.round(top),
      width: Math.max(2, Math.round(right - left)),
      height: Math.max(2, Math.round(bottom - top)),
    };
  }

  const ratio = initial.width / Math.max(1, initial.height);
  const centerX = initial.x + initial.width / 2;
  const centerY = initial.y + initial.height / 2;
  let width = Math.max(2, right - left);
  let height = Math.max(2, bottom - top);
  if ((west || east) && (north || south)) {
    if (Math.abs(width - initial.width) / initial.width >= Math.abs(height - initial.height) / initial.height) {
      height = width / ratio;
    } else {
      width = height * ratio;
    }
  } else if (west || east) {
    height = width / ratio;
  } else {
    width = height * ratio;
  }

  const anchorX = west ? initial.x + initial.width : east ? initial.x : centerX;
  const anchorY = north ? initial.y + initial.height : south ? initial.y : centerY;
  const maxWidth = west ? anchorX : east ? bounds.width - anchorX : 2 * Math.min(anchorX, bounds.width - anchorX);
  const maxHeight = north ? anchorY : south ? bounds.height - anchorY : 2 * Math.min(anchorY, bounds.height - anchorY);
  const fit = Math.min(1, maxWidth / width, maxHeight / height);
  width = Math.max(2, width * fit);
  height = Math.max(2, height * fit);
  left = west ? anchorX - width : east ? anchorX : anchorX - width / 2;
  top = north ? anchorY - height : south ? anchorY : anchorY - height / 2;
  return {
    x: Math.round(clamp(left, 0, bounds.width - width)),
    y: Math.round(clamp(top, 0, bounds.height - height)),
    width: Math.max(2, Math.round(width)),
    height: Math.max(2, Math.round(height)),
  };
}

export function recordingFileStem(path: string): string {
  const filename = path.split(/[\\/]/).at(-1) || "Captures_recording";
  return filename.replace(/\.[^.]+$/, "") || "Captures_recording";
}

export function recordingFilenameError(fileStem: string): string {
  const trimmed = fileStem.trim();
  const reserved = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i;
  const forbidden = '<>:"/\\|?*';
  const hasForbiddenCharacter = Array.from(trimmed).some((character) => (
    character.charCodeAt(0) < 32 || forbidden.includes(character)
  ));
  if (
    !trimmed
    || trimmed !== fileStem
    || trimmed === "."
    || trimmed === ".."
    || hasForbiddenCharacter
    || /[. ]$/.test(trimmed)
    || reserved.test(trimmed)
  ) {
    return "Enter a filename without folders or reserved characters.";
  }
  return "";
}

export function formatEditorTime(milliseconds: number, duration: number): string {
  const safe = Math.max(0, Math.round(milliseconds));
  const minutes = Math.floor(safe / 60_000);
  const seconds = Math.floor((safe % 60_000) / 1_000);
  if (duration < 60_000) {
    return `${minutes}:${String(seconds).padStart(2, "0")}.${String(safe % 1_000).padStart(3, "0")}`;
  }
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

export function timelineKeyboardDelta(key: string, duration: number): number | null {
  const step = duration < 60_000 ? 1 : 10;
  if (key === "ArrowLeft" || key === "ArrowDown") return -step;
  if (key === "ArrowRight" || key === "ArrowUp") return step;
  if (key === "PageDown") return -1_000;
  if (key === "PageUp") return 1_000;
  return null;
}
