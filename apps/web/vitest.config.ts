import { defineConfig } from "vitest/config";

// Unit tests must not load the app plugins or fetch GitHub history.
export default defineConfig({
  root: import.meta.dirname,
  test: {
    name: "web",
    environment: "node",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    isolate: true,
  },
});
