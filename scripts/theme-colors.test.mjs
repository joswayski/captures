import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sharedCssPath = resolve(root, "shared/themes.css");
const sharedTsPath = resolve(root, "shared/themes.ts");
const desktopCssPath = resolve(root, "apps/desktop/ui/src/styles.css");
const webCssPath = resolve(root, "apps/web/src/index.css");
const rustModelsPath = resolve(root, "apps/desktop/src-tauri/src/models.rs");

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

test("first-run setup uses the website palette instead of mustard cream", async () => {
  const desktopCss = await readFile(desktopCssPath, "utf8");
  const onboarding = desktopCss.match(/\.onboarding-shell\s*\{([\s\S]*?)\n\}/u)?.[1];
  assert.ok(onboarding, "missing .onboarding-shell palette");
  assert.match(onboarding, /--onboarding-canvas:\s*#f5f7fb/u);
  assert.match(onboarding, /--onboarding-border:\s*#e2e7f0/u);
  assert.match(onboarding, /--onboarding-accent:\s*#18181b/u);
  assert.doesNotMatch(onboarding, /var\(--theme-accent\)/u);
  assert.doesNotMatch(desktopCss, /\.onboarding-actions\s*\{[^}]*justify-content:\s*flex-start/u);
  assert.match(desktopCss, /\.onboarding-actions\s*\{[^}]*justify-content:\s*flex-end/u);
});

test("desktop and web surfaces consume the shared theme source", async () => {
  const [desktopCss, webCss] = await Promise.all([
    readFile(desktopCssPath, "utf8"),
    readFile(webCssPath, "utf8"),
  ]);

  assert.match(desktopCss, /@import "\.\.\/\.\.\/\.\.\/\.\.\/shared\/themes\.css";/u);
  assert.match(webCss, /@import "\.\.\/\.\.\/\.\.\/shared\/themes\.css";/u);
});

test("preferences keeps presets compact and gives Custom a full-spectrum treatment", async () => {
  const desktopCss = await readFile(desktopCssPath, "utf8");

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

test("preset accent and signal values are not duplicated outside the shared palette", async () => {
  const [sharedCss, desktopCss, webCss] = await Promise.all([
    readFile(sharedCssPath, "utf8"),
    readFile(desktopCssPath, "utf8"),
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
