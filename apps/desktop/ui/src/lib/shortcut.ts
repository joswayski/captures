export interface ShortcutKeyEvent {
  code: string;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

export type ShortcutRecordingResult =
  | { kind: "cancel" }
  | { kind: "waiting"; keys: string[] }
  | { kind: "complete"; keys: string[]; shortcut: string }
  | { kind: "invalid"; keys: string[]; message: string };

const MODIFIER_CODES = new Set([
  "AltLeft",
  "AltRight",
  "ControlLeft",
  "ControlRight",
  "MetaLeft",
  "MetaRight",
  "OSLeft",
  "OSRight",
  "ShiftLeft",
  "ShiftRight",
]);

const SUPPORTED_NAMED_CODES = new Set([
  "AudioVolumeDown",
  "AudioVolumeMute",
  "AudioVolumeUp",
  "Backquote",
  "Backslash",
  "Backspace",
  "BracketLeft",
  "BracketRight",
  "CapsLock",
  "Comma",
  "Delete",
  "End",
  "Enter",
  "Equal",
  "Home",
  "Insert",
  "MediaPause",
  "MediaPlay",
  "MediaPlayPause",
  "MediaStop",
  "MediaTrackNext",
  "MediaTrackPrevious",
  "Minus",
  "NumLock",
  "NumpadAdd",
  "NumpadDecimal",
  "NumpadDivide",
  "NumpadEnter",
  "NumpadEqual",
  "NumpadMultiply",
  "NumpadSubtract",
  "PageDown",
  "PageUp",
  "Pause",
  "Period",
  "PrintScreen",
  "Quote",
  "ScrollLock",
  "Semicolon",
  "Slash",
  "Space",
  "Tab",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
]);

export type ShortcutPlatform = "macos" | "windows" | "linux";

const SHARED_DISPLAY_NAMES: Record<string, string> = {
  control: "Ctrl",
  ctrl: "Ctrl",
  shift: "Shift",
  Backquote: "`",
  Backslash: "\\",
  BracketLeft: "[",
  BracketRight: "]",
  Comma: ",",
  Equal: "=",
  Minus: "-",
  Period: ".",
  Quote: "'",
  Semicolon: ";",
  Slash: "/",
  Space: "Space",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  ArrowUp: "↑",
  NumpadAdd: "Num +",
  NumpadDecimal: "Num .",
  NumpadDivide: "Num /",
  NumpadEnter: "Num Enter",
  NumpadEqual: "Num =",
  NumpadMultiply: "Num ×",
  NumpadSubtract: "Num -",
};

const MACOS_DISPLAY_NAMES: Record<string, string> = {
  ...SHARED_DISPLAY_NAMES,
  alt: "Option",
  option: "Option",
  command: "Cmd",
  cmd: "Cmd",
  super: "Cmd",
  meta: "Cmd",
  commandorcontrol: "Cmd",
  commandorctrl: "Cmd",
  cmdorcontrol: "Cmd",
  cmdorctrl: "Cmd",
  Backspace: "Delete",
  Enter: "Return",
};

const WINDOWS_DISPLAY_NAMES: Record<string, string> = {
  ...SHARED_DISPLAY_NAMES,
  alt: "Alt",
  option: "Alt",
  command: "Win",
  cmd: "Win",
  super: "Win",
  meta: "Win",
  commandorcontrol: "Ctrl",
  commandorctrl: "Ctrl",
  cmdorcontrol: "Ctrl",
  cmdorctrl: "Ctrl",
  Backspace: "Backspace",
  Enter: "Enter",
};

const LINUX_DISPLAY_NAMES: Record<string, string> = {
  ...WINDOWS_DISPLAY_NAMES,
  command: "Super",
  cmd: "Super",
  super: "Super",
  meta: "Super",
};

export function detectShortcutPlatform(
  userAgent = typeof navigator === "undefined" ? "" : navigator.userAgent,
): ShortcutPlatform {
  if (/Mac(?:intosh| OS X)/i.test(userAgent)) return "macos";
  if (/Win(?:dows)?/i.test(userAgent)) return "windows";
  return "linux";
}

function displayNamesFor(platform: ShortcutPlatform): Record<string, string> {
  if (platform === "macos") return MACOS_DISPLAY_NAMES;
  if (platform === "windows") return WINDOWS_DISPLAY_NAMES;
  return LINUX_DISPLAY_NAMES;
}

function modifierRequirementMessage(platform: ShortcutPlatform): string {
  if (platform === "macos") return "Include Ctrl, Shift, Option, or Command.";
  if (platform === "windows") return "Include Ctrl, Shift, Alt, or Win.";
  return "Include Ctrl, Shift, Alt, or Super.";
}

export function isModifierCode(code: string): boolean {
  return MODIFIER_CODES.has(code);
}

export function isSupportedShortcutCode(code: string): boolean {
  return /^(?:Digit[0-9]|Key[A-Z]|Numpad[0-9]|F(?:[1-9]|1[0-9]|2[0-4]))$/.test(code)
    || SUPPORTED_NAMED_CODES.has(code);
}

function canonicalModifiers(event: ShortcutKeyEvent): string[] {
  const modifiers: string[] = [];
  if (event.ctrlKey) modifiers.push("Control");
  if (event.shiftKey) modifiers.push("Shift");
  if (event.altKey) modifiers.push("Alt");
  if (event.metaKey) modifiers.push("Super");
  return modifiers;
}

export function modifierDisplayTokens(
  event: ShortcutKeyEvent,
  platform: ShortcutPlatform = detectShortcutPlatform(),
): string[] {
  return canonicalModifiers(event).map((token) => displayShortcutToken(token, platform));
}

export function displayShortcutToken(
  token: string,
  platform: ShortcutPlatform = detectShortcutPlatform(),
): string {
  const names = displayNamesFor(platform);
  const normalized = token.trim();
  const named = names[normalized] ?? names[normalized.toLowerCase()];
  if (named) return named;
  if (/^Key[A-Z]$/i.test(normalized)) return normalized.slice(3).toUpperCase();
  if (/^Digit[0-9]$/i.test(normalized)) return normalized.slice(5);
  if (/^Numpad[0-9]$/i.test(normalized)) return `Num ${normalized.slice(6)}`;
  return normalized;
}

export function shortcutDisplayTokens(
  shortcut: string,
  platform: ShortcutPlatform = detectShortcutPlatform(),
): string[] {
  return shortcut
    .split("+")
    .map((token) => displayShortcutToken(token, platform))
    .filter(Boolean);
}

export function recordShortcut(
  event: ShortcutKeyEvent,
  platform: ShortcutPlatform = detectShortcutPlatform(),
): ShortcutRecordingResult {
  if (event.code === "Escape") return { kind: "cancel" };

  const modifiers = canonicalModifiers(event);
  const modifierKeys = modifiers.map((token) => displayShortcutToken(token, platform));
  if (isModifierCode(event.code)) return { kind: "waiting", keys: modifierKeys };

  const key = displayShortcutToken(event.code, platform);
  if (!isSupportedShortcutCode(event.code)) {
    return {
      kind: "invalid",
      keys: [...modifierKeys, key],
      message: "That key cannot be used as a global shortcut.",
    };
  }
  if (modifiers.length === 0) {
    return {
      kind: "invalid",
      keys: [key],
      message: modifierRequirementMessage(platform),
    };
  }

  return {
    kind: "complete",
    keys: [...modifierKeys, key],
    shortcut: [...modifiers, event.code].join("+"),
  };
}
