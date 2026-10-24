import { readFile } from "node:fs/promises";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  root: import.meta.dirname,
  plugins: [
    react(),
    {
      name: "bundled-font-license",
      async generateBundle() {
        this.emitFile({
          type: "asset",
          fileName: "licenses/Nunito-OFL.txt",
          source: await readFile(
            new URL("../../../crates/captures-app/fonts/nunito/OFL.txt", import.meta.url),
            "utf8",
          ),
        });
      },
    },
  ],
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
  },
  test: {
    name: "desktop",
    isolate: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    globals: true,
  },
});
