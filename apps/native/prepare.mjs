// Build-time only. The native binary loads JSON/images, never CSS or JavaScript.
import { readFile, mkdir, writeFile, copyFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildThumbnailDustParticles, thumbnailDustVisualAt } from '../desktop/ui/src/lib/thumbnailExit.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));
const defaultOutput = resolve(root, 'apps/native/macos/Sources/CapturesNative/Resources');
const defaultTestOutput = resolve(root, 'apps/native/macos/Tests/CapturesNativeTests/Resources');
export const themes = ['mustard', 'ember', 'rose', 'violet', 'cobalt', 'aqua', 'mint', 'lime', 'mono'];

export function parseArguments(args) {
  if (args.length === 0) {
    return { destination: defaultOutput, testDestination: defaultTestOutput };
  }

  let destination;
  let testDestination;
  for (let i = 0; i < args.length; i += 2) {
    const option = args[i];
    const value = args[i + 1];
    if (!['--output', '--test-output'].includes(option)) {
      throw new Error(`Unknown argument ${option}`);
    }
    if (!value || value.startsWith('--')) {
      throw new Error(`Missing path for ${option}`);
    }
    if (option === '--output') {
      if (destination) throw new Error('Duplicate argument --output');
      destination = resolve(value);
    } else {
      if (testDestination) throw new Error('Duplicate argument --test-output');
      testDestination = resolve(value);
    }
  }
  if (!destination) throw new Error('--output is required when passing arguments');
  return { destination, testDestination };
}

// Deliberately constrained to the flat token sheets, not a general CSS parser.
// Fail on new syntax rather than silently dropping a future design token.
export function declarations(css, appearance, theme) {
  const result = {};
  const clean = css.replace(/\/\*[\s\S]*?\*\//g, '');
  let end = 0;
  for (const block of clean.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (clean.slice(end, block.index).trim()) throw new Error('Unsupported token CSS');
    end = block.index + block[0].length;
    const selectors = block[1].split(',').map(s => s.trim());
    for (const s of selectors) {
      if (!/^(:root|\[data-(appearance|capture-theme)="[a-z]+"\])$/.test(s)) {
        throw new Error(`Unsupported token selector ${s}`);
      }
    }
    // :root in the default dark rule is overridden by the later light rule,
    // just as in the shipping sheets.
    if (!selectors.some(s => s === ':root' || s === `[data-appearance="${appearance}"]`
      || s === `[data-capture-theme="${theme}"]`)) continue;
    for (const declaration of block[2].split(';').map(s => s.trim()).filter(Boolean)) {
      const match = /^(--[\w-]+|color-scheme)\s*:\s*([\s\S]+)$/.exec(declaration);
      if (!match) throw new Error(`Unsupported token declaration ${declaration}`);
      if (match[1].startsWith('--')) result[match[1].slice(2)] = match[2].replace(/\s+/g, ' ');
    }
  }
  if (clean.slice(end).trim()) throw new Error('Unsupported token CSS');
  return result;
}

export function resolveTokens(values) {
  function resolveValue(name, visited = []) {
    if (visited.includes(name)) throw new Error(`Token cycle: ${[...visited, name].join(' → ')}`);
    if (!(name in values)) throw new Error(`Unknown token ${name}`);
    const value = values[name].replace(/var\(--([\w-]+)\)/g,
      (_, dependency) => resolveValue(dependency, [...visited, name]));
    if (value.includes('var(')) throw new Error(`Unsupported variable expression in ${name}`);
    return value;
  }
  return Object.fromEntries(Object.keys(values).map(name => [name, resolveValue(name)]));
}

export function color(value) {
  if (/^#[\da-f]{6}$/i.test(value)) {
    return [1, 3, 5].map(i => parseInt(value.slice(i, i + 2), 16) / 255).concat(1);
  }
  const match = /^rgba?\(([^)]+)\)$/.exec(value);
  if (!match) return null;
  const parts = match[1].split(',').map(Number);
  if (![3, 4].includes(parts.length) || parts.some(n => !Number.isFinite(n))) {
    throw new Error(`Invalid color ${value}`);
  }
  return parts.slice(0, 3).map(n => n / 255).concat(parts[3] ?? 1);
}

export function particleFixture() {
  let seed = 739;
  const particles = buildThumbnailDustParticles(284, 160, {
    random: () => ((seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0) / 2 ** 32),
    originX: 22.5, originY: 22.5, imageWidth: 397, imageHeight: 251,
  });
  const times = [...new Set([-1, 0, 1, 110, 204, 420, 777, 1307, 1800, 2550,
    ...particles.flatMap(p => [p.delayMs - .01, p.delayMs, p.delayMs + .01])])];
  return { particles, times, poses: times.map(t => particles.map(p => thumbnailDustVisualAt(p, t))) };
}

export async function prepare(destination, testDestination) {
  const design = await readFile(resolve(root, 'shared/design.css'), 'utf8');
  const palette = await readFile(resolve(root, 'shared/themes.css'), 'utf8');
  const variants = {};
  for (const appearance of ['light', 'dark']) {
    for (const theme of themes) {
      const raw = resolveTokens({ ...declarations(design, appearance, theme), ...declarations(palette, appearance, theme) });
      variants[`${appearance}-${theme}`] = {
        colors: Object.fromEntries(Object.entries(raw).flatMap(([key, value]) => color(value) ? [[key, color(value)]] : [])),
        numbers: Object.fromEntries(Object.entries(raw).flatMap(([key, value]) => /^-?[\d.]+(px|ms)?$/.test(value) ? [[key, parseFloat(value)]] : [])),
        raw,
      };
    }
  }
  await mkdir(destination, { recursive: true });
  await writeFile(resolve(destination, 'tokens.json'), JSON.stringify(variants));
  const fixture = particleFixture();
  await writeFile(resolve(destination, 'dust.json'), JSON.stringify({ particles: fixture.particles }));
  if (testDestination) {
    await mkdir(testDestination, { recursive: true });
    await writeFile(resolve(testDestination, 'poses.json'), JSON.stringify(fixture));
  }
  // Existing product asset; do not introduce an independent icon design.
  await copyFile(resolve(root, 'apps/desktop/assets/icon.svg'), resolve(destination, 'icon.svg'));
  const liberationNotice = await readFile(resolve(root, 'crates/captures-app/fonts/liberation/LICENSE'), 'utf8');
  const nunitoNotice = await readFile(resolve(root, 'crates/captures-app/fonts/nunito/OFL.txt'), 'utf8');
  await writeFile(resolve(destination, 'EDITOR-FONT-LICENSE.txt'), `${liberationNotice}\n\n${nunitoNotice}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const { destination, testDestination } = parseArguments(process.argv.slice(2));
  await prepare(destination, testDestination);
}
