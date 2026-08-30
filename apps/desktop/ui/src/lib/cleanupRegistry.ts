export type Cleanup = () => void;

/**
 * Collects cleanup callbacks that can arrive after their owner has unmounted.
 *
 * Tauri listener registration is asynchronous. A React effect can therefore
 * dispose before `listen()` resolves; callbacks registered after that point
 * must be cleaned up immediately instead of being left attached to the native
 * event bridge.
 */
export function createCleanupRegistry() {
  let disposed = false;
  let registered: Cleanup[] = [];

  return {
    add(...cleanups: Cleanup[]): boolean {
      if (disposed) {
        cleanups.forEach((cleanup) => cleanup());
        return false;
      }
      registered.push(...cleanups);
      return true;
    },

    dispose(): void {
      if (disposed) return;
      disposed = true;
      const cleanups = registered;
      registered = [];
      cleanups.forEach((cleanup) => cleanup());
    },
  };
}
