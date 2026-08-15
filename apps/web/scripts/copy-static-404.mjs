import { copyFile, mkdir } from "node:fs/promises";

const outputDirectory = new URL("../dist/client/", import.meta.url);

await mkdir(outputDirectory, { recursive: true });
await copyFile(
  new URL("../static/404.html", import.meta.url),
  new URL("404.html", outputDirectory),
);
