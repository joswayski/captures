import { play, set, type CueName, type PlayOptions } from "@foleyjs/core";

const STORAGE_KEY = "captures-interaction-sounds";
const CHANGE_EVENT = "captures-sounds-changed";
const controls = 'button, a[href], input, select, textarea, [role="button"], [role="tab"], [role="radio"], [role="option"], [role="switch"], [role="checkbox"]';

export function soundsEnabled() {
  try {
    return localStorage.getItem(STORAGE_KEY) !== "off";
  } catch {
    return true;
  }
}

export function subscribeSounds(callback: () => void) {
  const onStorage = (event: StorageEvent) => {
    if (event.key === STORAGE_KEY || event.key === null) callback();
  };
  window.addEventListener("storage", onStorage);
  window.addEventListener(CHANGE_EVENT, callback);
  return () => {
    window.removeEventListener("storage", onStorage);
    window.removeEventListener(CHANGE_EVENT, callback);
  };
}

export function setSoundsEnabled(enabled: boolean) {
  localStorage.setItem(STORAGE_KEY, enabled ? "on" : "off");
  window.dispatchEvent(new Event(CHANGE_EVENT));
  if (enabled) playSound("on");
}

export function playSound(cue: CueName, options?: PlayOptions) {
  if (!soundsEnabled()) return;
  // Audio is an enhancement: unavailable/blocked Web Audio must not break an action.
  try {
    play(cue, options);
  } catch {
    // Native webviews and browser autoplay policies can deny audio.
  }
}

function controlFor(target: EventTarget | null) {
  if (!(target instanceof Element)) return null;
  const control = target.closest<HTMLElement>(controls);
  if (!control || control.closest('[disabled], [aria-disabled="true"], [inert], [data-sound="off"]')) return null;
  return control;
}

/** Delegated once per webview, including portals and controls mounted later. */
export function installInteractionSounds() {
  set({ theme: "soft", volume: 0.38, space: 0.12, transpose: -2 });
  const sync = () => set({ muted: !soundsEnabled() });
  sync();
  const unsubscribe = subscribeSounds(sync);
  let lastTick = -Infinity;
  const tick = () => {
    if (performance.now() - lastTick < 90) return;
    lastTick = performance.now();
    playSound("tick", { volume: 0.35 });
  };
  const click = (event: MouseEvent) => {
    const control = controlFor(event.target);
    if (!control) return;
    // Native checkbox activation already updated checked; aria controls have not
    // reached React's handler yet. Labels forward their click to the input.
    if (control instanceof HTMLInputElement) {
      if (control.type === "checkbox") playSound(control.checked ? "on" : "off");
      else if (control.type === "radio") playSound("switch");
      return;
    }
    if (control.matches("textarea, select")) return;
    if (control.matches('[role="switch"], [role="checkbox"]')) {
      playSound(control.getAttribute("aria-checked") === "true" ? "off" : "on");
    } else if (control.matches('[role="radio"], [role="tab"], [role="option"], [aria-pressed]')) {
      playSound("switch");
    } else {
      playSound("tap");
    }
  };
  const input = (event: Event) => {
    const control = controlFor(event.target);
    if (control?.matches('input[type="range"], input[type="number"]')) tick();
  };
  const change = (event: Event) => {
    if (controlFor(event.target)?.matches("select")) playSound("switch");
  };
  const focus = (event: FocusEvent) => {
    const control = controlFor(event.target);
    if (control?.matches(":focus-visible")) tick();
  };
  // Canvas gestures get one tactile start/end, never sound on every drag frame.
  const gestures = new Set<number>();
  const pointerDown = (event: PointerEvent) => {
    if (event.button !== 0 || !(event.target instanceof Element)) return;
    if (!event.target.closest("[data-sound-gesture]") || event.target.closest(controls)) return;
    gestures.add(event.pointerId);
    playSound("press", { volume: 0.5 });
  };
  const pointerUp = (event: PointerEvent) => {
    if (gestures.delete(event.pointerId)) playSound("drop", { volume: 0.5 });
  };
  const pointerCancel = (event: PointerEvent) => { gestures.delete(event.pointerId); };
  // Errors sound when newly announced, not when a window initially renders.
  const alerts = new WeakMap<Element, string>();
  document.querySelectorAll('[role="alert"]').forEach((el) => alerts.set(el, el.textContent ?? ""));
  const observer = new MutationObserver(() => {
    let changed = false;
    document.querySelectorAll('[role="alert"]').forEach((el) => {
      const message = el.textContent?.trim() ?? "";
      if (message && alerts.get(el) !== message) changed = true;
      alerts.set(el, message);
    });
    if (changed) playSound("error");
  });
  observer.observe(document.body, { subtree: true, childList: true, characterData: true });
  document.addEventListener("click", click, true);
  document.addEventListener("input", input, true);
  document.addEventListener("change", change, true);
  document.addEventListener("focusin", focus);
  document.addEventListener("pointerdown", pointerDown, true);
  document.addEventListener("pointerup", pointerUp, true);
  document.addEventListener("pointercancel", pointerCancel, true);
  return () => {
    unsubscribe();
    observer.disconnect();
    document.removeEventListener("click", click, true);
    document.removeEventListener("input", input, true);
    document.removeEventListener("change", change, true);
    document.removeEventListener("focusin", focus);
    document.removeEventListener("pointerdown", pointerDown, true);
    document.removeEventListener("pointerup", pointerUp, true);
    document.removeEventListener("pointercancel", pointerCancel, true);
  };
}
