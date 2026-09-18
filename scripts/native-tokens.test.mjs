import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { declarations, resolveTokens, color, themes, particleFixture } from '../apps/native/prepare.mjs';

test('native tokens preserve light overrides, palette overrides and fixed media colors', async () => {
  const design = await readFile(new URL('../shared/design.css', import.meta.url), 'utf8');
  const palette = await readFile(new URL('../shared/themes.css', import.meta.url), 'utf8');
  const tokens = (appearance, theme) => resolveTokens({
    ...declarations(design, appearance, theme), ...declarations(palette, appearance, theme),
  });
  assert.equal(tokens('light', 'cobalt')['surface-raised'], '#ffffff');
  assert.equal(tokens('dark', 'cobalt')['surface-raised'], '#16161b');
  assert.equal(tokens('light', 'cobalt')['theme-accent'], '#2563eb');
  assert.equal(tokens('dark', 'mustard')['theme-accent'], '#ffca28');
  for (const theme of themes) {
    const light = tokens('light', theme), dark = tokens('dark', theme);
    assert.equal(light.glass, dark.glass);
    assert.equal(light['glass-text'], dark['glass-text']);
    assert.ok(!Object.values(light).some(v => v.includes('var(')));
    assert.equal(light['thumbnail-drop-reject-duration'], '420ms');
  }
  assert.deepEqual(color(tokens('light', 'cobalt')['surface-selected']), [37 / 255, 99 / 255, 235 / 255, .13]);
  assert.deepEqual(color('#ffca28'), [1, 202 / 255, 40 / 255, 1]);
});

test('unsupported CSS, missing variables and cycles fail rather than silently changing native colors', () => {
  assert.throws(() => declarations('@media (dark) { :root { --a: 1; } }', 'dark', 'mustard'));
  assert.throws(() => declarations('.widget { --a: 1; }', 'dark', 'mustard'));
  assert.throws(() => resolveTokens({ a: 'var(--b)', b: 'var(--a)' }), /cycle/);
  assert.throws(() => resolveTokens({ a: 'var(--missing)' }), /Unknown/);
});

test('shared dust fixtures include asymmetric delay boundaries and complete end poses', () => {
  const fixture = particleFixture();
  assert.equal(fixture.particles.length, 198);
  assert.deepEqual(fixture, particleFixture());
  const end = fixture.poses[fixture.times.indexOf(2550)];
  fixture.particles.forEach((p, i) => {
    assert.ok(fixture.times.includes(p.delayMs - .01));
    assert.ok(fixture.times.includes(p.delayMs + .01));
    assert.equal(end[i].opacity, 0);
    assert.ok(Math.abs(end[i].dx - p.dx) < 1e-8);
  });
});
