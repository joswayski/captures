import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('.', import.meta.url));
export default defineConfig({
  root: `${root}web`, publicDir: `${root}.build/public`,
  build: { outDir: `${root}.build/web`, emptyOutDir: true },
  server: { host: '0.0.0.0', port: 1432, strictPort: true, fs: { allow: [`${root}../..`] } },
});
