import { invoke as coreInvoke, isTauri as coreIsTauri } from "@tauri-apps/api/core";
import { emit as coreEmit, listen as coreListen } from "@tauri-apps/api/event";

const bridge = {
  invoke: coreInvoke,
  listen: coreListen,
  emit: coreEmit,
  isTauri: coreIsTauri,
};

/** Swap Tauri IPC for the browser preview used to screenshot every surface. */
export function installTauriBridge(next: Partial<typeof bridge>) {
  Object.assign(bridge, next);
}

export const invoke: typeof coreInvoke = ((...args: Parameters<typeof coreInvoke>) => (
  bridge.invoke(...args)
)) as typeof coreInvoke;

export const listen: typeof coreListen = ((...args: Parameters<typeof coreListen>) => (
  bridge.listen(...args)
)) as typeof coreListen;

export const emit: typeof coreEmit = ((...args: Parameters<typeof coreEmit>) => (
  bridge.emit(...args)
)) as typeof coreEmit;

export function isTauri() {
  return bridge.isTauri();
}
