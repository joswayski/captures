import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sharedCssPath = resolve(root, "shared/themes.css");
const sharedTsPath = resolve(root, "shared/themes.ts");
const designCssPath = resolve(root, "shared/design.css");
const appearanceTsPath = resolve(root, "shared/appearance.ts");
const desktopEntryCssPath = resolve(root, "apps/desktop/ui/src/styles.css");
const desktopStylesDir = resolve(root, "apps/desktop/ui/src/styles");
const webCssPath = resolve(root, "apps/web/src/index.css");
const rustModelsPath = resolve(root, "apps/desktop/src-tauri/src/models.rs");

async function readDesktopCss() {
  const entry = await readFile(desktopEntryCssPath, "utf8");
  const files = (await readdir(desktopStylesDir)).filter((name) => name.endsWith(".css")).sort();
  const modules = await Promise.all(
    files.map((name) => readFile(join(desktopStylesDir, name), "utf8")),
  );
  return [entry, ...modules].join("\n");
}

test("every declared color theme has a shared CSS palette and backend value", async () => {
  const [sharedCss, sharedTs, rustModels] = await Promise.all([
    readFile(sharedCssPath, "utf8"),
    readFile(sharedTsPath, "utf8"),
    readFile(rustModelsPath, "utf8"),
  ]);
  const ids = [...sharedTs.matchAll(/\bid:\s*"([^"]+)"/gu)].map((match) => match[1]);
  const rustEnum = rustModels.match(/pub enum ColorTheme\s*\{([^}]+)\}/u)?.[1];
  assert.ok(rustEnum, "missing Rust ColorTheme enum");
  const backendIds = [...rustEnum.matchAll(/^\s*([A-Z][A-Za-z]+),$/gmu)]
    .map((match) => match[1].replaceAll(/([a-z0-9])([A-Z])/gu, "$1_$2").toLowerCase());

  assert.deepEqual(ids, [
    "mustard",
    "ember",
    "rose",
    "violet",
    "cobalt",
    "aqua",
    "mint",
    "lime",
    "mono",
    "custom",
  ]);
  assert.deepEqual(backendIds, ids);
  for (const id of ids) {
    assert.match(sharedCss, new RegExp(`data-capture-theme="${id}"`, "u"));
  }
});

test("every appearance mode has design tokens and a backend value", async () => {
  const [designCss, appearanceTs, rustModels] = await Promise.all([
    readFile(designCssPath, "utf8"),
    readFile(appearanceTsPath, "utf8"),
    readFile(rustModelsPath, "utf8"),
  ]);
  const ids = [...appearanceTs.matchAll(/\bid:\s*"([^"]+)"/gu)].map((match) => match[1]);
  const rustEnum = rustModels.match(/pub enum Appearance\s*\{([^}]+)\}/u)?.[1];
  assert.ok(rustEnum, "missing Rust Appearance enum");
  const backendIds = [...rustEnum.matchAll(/^\s*([A-Z][A-Za-z]+),$/gmu)]
    .map((match) => match[1].toLowerCase());

  assert.deepEqual(ids, ["system", "light", "dark"]);
  assert.deepEqual(backendIds, ids);
  assert.match(designCss, /\[data-appearance="light"\]/u);
  assert.match(designCss, /\[data-appearance="dark"\]/u);
});

test("light and dark declare the same semantic surface tokens", async () => {
  const designCss = await readFile(designCssPath, "utf8");
  const block = (selector) => {
    const start = designCss.indexOf(selector);
    assert.notEqual(start, -1, `missing ${selector} block`);
    const open = designCss.indexOf("{", start);
    const close = designCss.indexOf("\n}", open);
    return designCss.slice(open, close);
  };
  const names = (text) => new Set(
    [...text.matchAll(/^\s*(--[a-z0-9-]+):/gmu)].map((match) => match[1]),
  );

  const dark = names(block(`[data-appearance="dark"]`));
  const light = names(block(`[data-appearance="light"]`));

  assert.ok(dark.size > 30, "expected a full dark token set");
  assert.deepEqual([...dark].sort(), [...light].sort());
  for (const required of [
    "--surface-canvas",
    "--surface-raised",
    "--surface-overlay",
    "--border",
    "--text",
    "--text-muted",
    "--solid",
    "--danger-text",
  ]) {
    assert.ok(dark.has(required), `missing ${required}`);
  }
});

test("desktop and web surfaces consume the shared theme source", async () => {
  const [entryCss, webCss] = await Promise.all([
    readFile(desktopEntryCssPath, "utf8"),
    readFile(webCssPath, "utf8"),
  ]);

  assert.match(entryCss, /@import "\.\.\/\.\.\/\.\.\/\.\.\/shared\/themes\.css";/u);
  assert.match(entryCss, /@import "\.\.\/\.\.\/\.\.\/\.\.\/shared\/design\.css";/u);
  assert.match(webCss, /@import "\.\.\/\.\.\/\.\.\/shared\/themes\.css";/u);
});

test("first-run setup follows the shared appearance tokens", async () => {
  const desktopCss = await readDesktopCss();
  const onboarding = desktopCss.match(/\.onboarding-shell\s*\{([\s\S]*?)\n\}/u)?.[1];
  assert.ok(onboarding, "missing .onboarding-shell rule");
  // The old design hardcoded a light island inside an otherwise dark app.
  assert.doesNotMatch(onboarding, /#[0-9a-f]{3,6}/iu);
  assert.match(onboarding, /background:\s*var\(--surface-canvas\)/u);
  assert.match(desktopCss, /\.onboarding-actions\s*\{[^}]*justify-content:\s*flex-end/u);
});

test("preferences keeps presets compact and gives Custom a full-spectrum treatment", async () => {
  const desktopCss = await readDesktopCss();

  assert.match(
    desktopCss,
    /\.theme-options\s*\{[^}]*grid-template-columns:\s*repeat\(5,/u,
  );
  assert.match(
    desktopCss,
    /\.theme-option-custom \.theme-option-preview\s*\{[^}]*linear-gradient/u,
  );
  assert.match(
    desktopCss,
    /\.custom-theme-editor::before\s*\{[^}]*linear-gradient/u,
  );
});

test("mini-preview stack controls use opaque glass tokens and contained shadows", async () => {
  const [designCss, desktopCss] = await Promise.all([
    readFile(designCssPath, "utf8"),
    readDesktopCss(),
  ]);
  assert.match(designCss, /--glass-strong-solid:\s*rgb\(15, 15, 18\)/u);
  assert.match(designCss, /--glass-raised-solid:\s*rgb\(38, 38, 45\)/u);
  assert.match(
    desktopCss,
    /html\[data-preview-harness-view="thumbnail"\] #root\s*\{[^}]*width:\s*340px/u,
  );

  const stackControl =
    desktopCss.match(/\.thumbnail-stack-control\s*\{([\s\S]*?)\n\}/u)?.[1];
  assert.ok(stackControl, "missing .thumbnail-stack-control rule");
  assert.match(stackControl, /background:\s*var\(--glass-strong-solid\)/u);
  assert.match(stackControl, /width:\s*28px/u);
  assert.doesNotMatch(stackControl, /rgb\(/u);
  assert.doesNotMatch(stackControl, /var\(--glass-shadow\)/u);
  assert.match(
    desktopCss,
    /--thumbnail-card-shadow:\s*0 6px 14px[\s\S]*?0 2px 5px/u,
  );
  assert.match(
    desktopCss,
    /\.thumbnail-stack-control:hover[\s\S]*?background:\s*var\(--glass-raised-solid\)/u,
  );
  assert.match(
    desktopCss,
    /\.thumbnail-stack-control:hover[\s\S]*?border-color:\s*var\(--glass-border-strong\)/u,
  );
  assert.match(
    desktopCss,
    /\.thumbnail-stack-minimize:hover[\s\S]*?width:\s*92px/u,
  );
  assert.match(
    desktopCss,
    /\.thumbnail-stack-minimize:hover \.thumbnail-stack-minimize-label[\s\S]*?opacity:\s*1/u,
  );
  assert.match(
    desktopCss,
    /\.thumbnail-stack-minimized > \.thumbnail-card[\s\S]*?translate3d/u,
  );
  assert.match(
    desktopCss,
    /\.thumbnail-stack-minimized > \.thumbnail-card[\s\S]*?-13px/u,
  );
  const stackToolbar =
    desktopCss.match(/\.thumbnail-stack-toolbar\s*\{([\s\S]*?)\n\}/u)?.[1];
  assert.ok(stackToolbar, "missing .thumbnail-stack-toolbar rule");
  assert.match(stackToolbar, /bottom:\s*16px/u);
  assert.match(stackToolbar, /left:\s*28px/u);
  assert.doesNotMatch(stackToolbar, /\btop:/u);
  assert.doesNotMatch(stackToolbar, /\bright:/u);
  assert.match(
    desktopCss,
    /\.thumbnail-stack-control\s*\{[\s\S]*?border-radius:\s*var\(--r-md\)/u,
  );
  assert.match(
    desktopCss,
    /\.thumbnail-stack-control\s*\{[\s\S]*?box-shadow:\s*var\(--thumbnail-card-shadow\)/u,
  );
  assert.doesNotMatch(desktopCss, /\.thumbnail-stack-clear/u);
});

test("preset accent and signal values are not duplicated outside the shared palette", async () => {
  const [sharedCss, desktopCss, webCss] = await Promise.all([
    readFile(sharedCssPath, "utf8"),
    readDesktopCss(),
    readFile(webCssPath, "utf8"),
  ]);
  const themedValues = [
    ...sharedCss.matchAll(
      /--theme-(?:accent(?:-hover|-strong|-readable|-text|-text-strong)?|signal(?:-hover|-strong|-deep|-text|-text-strong)?):\s*(#[0-9a-f]{6})/giu,
    ),
  ].map((match) => match[1].toLowerCase());
  const consumers = `${desktopCss}\n${webCss}`.toLowerCase();

  for (const value of new Set(themedValues)) {
    assert.equal(
      consumers.includes(value),
      false,
      `${value} must only be declared in shared/themes.css`,
    );
  }
});

test("theme action colors preserve readable text contrast", async () => {
  const sharedCss = await readFile(sharedCssPath, "utf8");
  const ids = [
    "mustard",
    "ember",
    "rose",
    "violet",
    "cobalt",
    "aqua",
    "mint",
    "lime",
    "mono",
    "custom",
  ];

  for (const id of ids) {
    const selectorStart = sharedCss.indexOf(`[data-capture-theme="${id}"]`);
    assert.notEqual(selectorStart, -1, `missing palette selector for ${id}`);
    const blockStart = sharedCss.indexOf("{", selectorStart);
    const blockEnd = sharedCss.indexOf("}", blockStart);
    assert.notEqual(blockStart, -1, `missing palette block start for ${id}`);
    assert.notEqual(blockEnd, -1, `missing palette block end for ${id}`);
    const block = sharedCss.slice(blockStart + 1, blockEnd);

    const value = (property) => {
      const color = block.match(new RegExp(`--${property}:\\s*(#[0-9a-f]{6})`, "iu"))?.[1];
      assert.ok(color, `missing --${property} for ${id}`);
      return color;
    };
    const accentInk = value("theme-accent-ink");
    const signalInk = value("theme-signal-ink");

    assert.ok(contrast(value("theme-accent"), accentInk) >= 4.5, `${id} accent contrast`);
    assert.ok(contrast(value("theme-accent-hover"), accentInk) >= 4.5, `${id} hover contrast`);
    assert.ok(
      contrast(value("theme-accent-readable"), value("theme-web-canvas")) >= 4.5,
      `${id} readable accent contrast`,
    );
    assert.ok(contrast(value("theme-signal"), signalInk) >= 4.5, `${id} signal contrast`);
  }
});

function contrast(first, second) {
  const [lighter, darker] = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (lighter + 0.05) / (darker + 0.05);
}

function luminance(hex) {
  const channels = [1, 3, 5].map((index) => Number.parseInt(hex.slice(index, index + 2), 16) / 255);
  const linear = channels.map((channel) => (
    channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
  ));
  return (0.2126 * linear[0]) + (0.7152 * linear[1]) + (0.0722 * linear[2]);
}
