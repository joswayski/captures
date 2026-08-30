import { createCleanupRegistry } from "./cleanupRegistry";

describe("createCleanupRegistry", () => {
  it("disposes registered callbacks exactly once", () => {
    const first = vi.fn();
    const second = vi.fn();
    const registry = createCleanupRegistry();

    expect(registry.add(first, second)).toBe(true);
    registry.dispose();
    registry.dispose();

    expect(first).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledTimes(1);
  });

  it("immediately disposes callbacks that register after teardown", () => {
    const late = vi.fn();
    const registry = createCleanupRegistry();

    registry.dispose();

    expect(registry.add(late)).toBe(false);
    expect(late).toHaveBeenCalledTimes(1);
  });
});
