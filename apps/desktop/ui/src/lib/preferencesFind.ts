import type { ShortcutPlatform } from "./shortcut";
import { eventMatchesShortcut } from "./shortcut";

/** Preference chrome that should participate in Cmd/Ctrl+F find. */
export const PREFERENCE_FIND_SELECTOR = [
  ".settings-card-header",
  ".setting-row",
  ".check-row",
  ".settings-utility-row",
  ".shortcut-row",
  ".custom-theme-editor",
].join(", ");

export type PreferencesFindCommand = "open" | "next" | "previous" | "close";

export function collectPreferenceFindTargets(root: ParentNode): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(PREFERENCE_FIND_SELECTOR)];
}

export function preferenceTextMatches(text: string, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return false;
  return text.replace(/\s+/g, " ").toLowerCase().includes(needle);
}

export function matchPreferenceFindTargets(
  targets: HTMLElement[],
  query: string,
): HTMLElement[] {
  return targets.filter((target) => preferenceTextMatches(target.textContent ?? "", query));
}

export function wrapFindIndex(count: number, current: number, delta: 1 | -1): number {
  if (count <= 0) return 0;
  return (current + delta + count) % count;
}

export function preferenceFindCountLabel(
  query: string,
  count: number,
  index: number,
): string {
  if (!query.trim()) return "";
  if (count === 0) return "No results";
  return `${index + 1} of ${count}`;
}

export type PreferencesFindKeyEvent = {
  key: string;
  code: string;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  defaultPrevented?: boolean;
  isComposing?: boolean;
  target: EventTarget | null;
};

function shortcutEvent(event: PreferencesFindKeyEvent) {
  return {
    code: event.code,
    ctrlKey: event.ctrlKey,
    shiftKey: event.shiftKey,
    altKey: event.altKey,
    metaKey: event.metaKey,
  };
}

function isFindField(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && Boolean(target.closest(".preferences-find"));
}

/** Map a key event to the in-window find command, using each OS’s find chord. */
export function preferencesFindCommand(
  event: PreferencesFindKeyEvent,
  platform: ShortcutPlatform,
  findOpen: boolean,
): PreferencesFindCommand | null {
  if (event.defaultPrevented || event.isComposing) return null;
  const keys = shortcutEvent(event);
  if (eventMatchesShortcut(keys, "CommandOrControl+KeyF", platform)) return "open";
  if (!findOpen) return null;
  if (event.key === "Escape" && !event.metaKey && !event.ctrlKey && !event.altKey) {
    return "close";
  }
  const inFindField = isFindField(event.target);
  if (
    eventMatchesShortcut(keys, "CommandOrControl+KeyG", platform)
    || (event.key === "F3" && !event.shiftKey && !event.metaKey && !event.ctrlKey && !event.altKey)
    || (inFindField && event.key === "Enter" && !event.shiftKey)
  ) {
    return "next";
  }
  if (
    eventMatchesShortcut(keys, "CommandOrControl+Shift+KeyG", platform)
    || (event.key === "F3" && event.shiftKey && !event.metaKey && !event.ctrlKey && !event.altKey)
    || (inFindField && event.key === "Enter" && event.shiftKey)
  ) {
    return "previous";
  }
  return null;
}
