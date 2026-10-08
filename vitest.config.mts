import { defineConfig } from "vitest/config";

// Explicit ESM without changing the runtime module mode of the root package.
export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: "scripts",
          include: ["scripts/**/*.test.mjs"],
          environment: "node",
          isolate: true,
        },
      },
      "apps/web/vitest.config.ts",
      "apps/desktop/ui/vite.config.ts",
    ],
  },
});
