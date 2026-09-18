import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import test from 'node:test';
import { buildCustomThemeVariables } from '../shared/themes.ts';

const fixtureUrl = new URL('../crates/captures-settings/tests/custom-theme-golden.json', import.meta.url);
const cases = [
  { accent: '#123abc', signal: '#de4567', light: false },
  { accent: '#123abc', signal: '#de4567', light: true },
  { accent: '#000000', signal: '#ffffff', light: false },
  { accent: '#ffffff', signal: '#000000', light: true },
  { accent: '#767676', signal: '#777777', light: false },
  { accent: '#767676', signal: '#777777', light: true },
];

function color(value) {
  if (/^#[\da-f]{6}$/iu.test(value)) {
    return [1, 3, 5].map(i => Number.parseInt(value.slice(i, i + 2), 16) / 255).concat(1);
  }
  const match = /^rgba\(([^)]+)\)$/u.exec(value);
  assert.ok(match, `unsupported shipping color ${value}`);
  const parts = match[1].split(',').map(Number);
  return parts.slice(0, 3).map(channel => channel / 255).concat(parts[3]);
}

function shippingCase(input) {
  const variables = buildCustomThemeVariables(input);
  const colors = Object.fromEntries(Object.entries(variables).flatMap(([name, value]) => {
    if (name.endsWith('-rgb')) return [];
    return [[name.slice(2), color(value)]];
  }));
  const accentRgb = variables['--theme-accent-rgb'];
  const signalRgb = variables['--theme-signal-rgb'];
  colors['surface-selected'] = color(`rgba(${accentRgb}, ${input.light ? 0.13 : 0.14})`);
  colors['danger-surface'] = color(input.light
    ? `rgba(${signalRgb}, 0.1)` : variables['--theme-signal-surface']);
  colors['danger-text'] = color(input.light
    ? variables['--theme-signal-strong'] : variables['--theme-signal-text']);
  colors['danger-border'] = color(`rgba(${signalRgb}, ${input.light ? 0.3 : 0.34})`);
  return { ...input, colors: Object.fromEntries(Object.entries(colors).sort()) };
}

// This is the fixture's provenance command. It runs shipping TypeScript rather
// than reimplementing its palette math in the test oracle.
if (process.argv.includes('--write')) {
  await writeFile(fixtureUrl, `${JSON.stringify(cases.map(shippingCase), null, 2)}\n`);
} else {
  test('native custom-theme golden vectors come from shipping TypeScript', async () => {
    const fixture = JSON.parse(await readFile(fixtureUrl, 'utf8'));
    assert.deepEqual(fixture, cases.map(shippingCase));
  });

  test('vectors cover ink thresholds, extremes, appearances and asymmetric channels', () => {
    const vectors = cases.map(shippingCase);
    assert.notDeepEqual(vectors[4].colors['theme-accent-ink'], vectors[4].colors['theme-signal-ink']);
    assert.notDeepEqual(vectors[0].colors['surface-selected'], vectors[1].colors['surface-selected']);
    assert.deepEqual(vectors[2].colors['theme-accent'], [0, 0, 0, 1]);
    assert.deepEqual(vectors[2].colors['theme-signal'], [1, 1, 1, 1]);
  });
}
