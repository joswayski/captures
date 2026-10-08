import { defineConfig } from "vitest/config";

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
