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
  PrintScreen: "PrtScn",
  printscreen: "PrtScn",
  prtscn: "PrtScn",
  print: "PrtScn",
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

function harnessShortcutPlatform(): ShortcutPlatform | undefined {
  if (typeof window === "undefined") return undefined;
  const value = new URLSearchParams(window.location.search).get("platform");
  if (value === "macos" || value === "windows" || value === "linux") return value;
  return undefined;
}

export function detectShortcutPlatform(
  userAgent = typeof navigator === "undefined" ? "" : navigator.userAgent,
): ShortcutPlatform {
  const harness = harnessShortcutPlatform();
  if (harness) return harness;
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
  if (platform === "macos") return "Include Ctrl, Shift, Option, or Command, or use Print Screen.";
  if (platform === "windows") return "Include Ctrl, Shift, Alt, or Win, or use Print Screen.";
  return "Include Ctrl, Shift, Alt, or Super, or use Print Screen.";
}

export function platformShortcutHelp(platform: ShortcutPlatform): {
  intro: string;
  takeoverTitle: string;
  takeoverBody: string;
} {
  if (platform === "macos") {
    return {
      intro:
        "Defaults match macOS Screenshot for full screen, region, and recording. Captures-only actions keep their own shortcuts.",
      takeoverTitle: "macOS Screenshot shortcuts",
      takeoverBody:
        "Captures unbinds overlapping Screenshot app keys (⌘⇧3, ⌘⇧4, ⌘⇧5) so they reach this app instead of the system overlay. Restore them in System Settings if you want both.",
    };
  }
  if (platform === "windows") {
    return {
      intro:
        "Defaults match Windows screenshot keys: Win+Shift+S region, PrtScn full screen, Alt+PrtScn window, and Win+Alt+R recording.",
      takeoverTitle: "Windows screenshot shortcuts",
      takeoverBody:
        "Captures unbinds overlapping Snipping Tool keys so Win+Shift+S and Print Screen reach this app instead of the system overlay. Restore them in Windows keyboard settings if you want both.",
    };
  }
  return {
    intro:
      "Defaults match GNOME/Ubuntu screenshot keys: PrtScn opens New Capture, Super+Shift+S region, Shift+PrtScn full screen, Alt+PrtScn window, and Ctrl+Shift+Alt+R recording.",
    takeoverTitle: "GNOME screenshot shortcuts",
    takeoverBody:
      "Captures turns off overlapping GNOME screenshot keys (and KDE Spectacle region capture when those tools are installed) so they reach this app. Restore them in Keyboard settings if you want both.",
  };
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

function canonicalShortcutToken(token: string): string {
  const normalized = token.trim().toLowerCase();
  if (normalized === "control" || normalized === "ctrl") return "control";
  if (normalized === "shift") return "shift";
  if (normalized === "alt" || normalized === "option") return "alt";
  if (
    normalized === "super"
    || normalized === "cmd"
    || normalized === "command"
    || normalized === "meta"
    || normalized === "win"
  ) {
    return "super";
  }
  if (
    normalized === "commandorcontrol"
    || normalized === "commandorctrl"
    || normalized === "cmdorctrl"
    || normalized === "cmdorcontrol"
  ) {
    return "commandorcontrol";
  }
  if (
    normalized === "printscreen"
    || normalized === "prtscn"
    || normalized === "prtsc"
    || normalized === "print"
  ) {
    return "printscreen";
  }
  if (normalized.startsWith("digit") && normalized.length === 6) return normalized.slice(5);
  if (normalized.startsWith("key") && normalized.length === 4) return normalized.slice(3);
  return normalized;
}

function expandCommandOrControl(
  modifiers: Set<string>,
  platform: ShortcutPlatform,
): Set<string> {
  const next = new Set(modifiers);
  if (next.delete("commandorcontrol")) {
    next.add(platform === "macos" ? "super" : "control");
  }
  return next;
}

/** True when a key event matches a stored global shortcut, including factory CommandOrControl chords. */
export function eventMatchesShortcut(
  event: ShortcutKeyEvent,
  shortcut: string,
  platform: ShortcutPlatform = detectShortcutPlatform(),
): boolean {
  const tokens = shortcut
    .split("+")
    .map(canonicalShortcutToken)
    .filter(Boolean);
  const key = tokens.pop();
  if (!key) return false;
  const expected = expandCommandOrControl(new Set(tokens), platform);
  const actual = new Set(canonicalModifiers(event).map(canonicalShortcutToken));
  if (canonicalShortcutToken(event.code) !== key || actual.size !== expected.size) {
    return false;
  }
  for (const modifier of expected) {
    if (!actual.has(modifier)) return false;
  }
  return true;
}

/** True for Escape regardless of leftover modifiers or `key` vs `code`. */
export function isCaptureEscapeKey(event: { key?: string; code?: string }): boolean {
  return event.key === "Escape" || event.key === "Esc" || event.code === "Escape";
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
  if (modifiers.length === 0 && event.code !== "PrintScreen") {
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
