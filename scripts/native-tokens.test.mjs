import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { declarations, resolveTokens, color, themes, particleFixture, parseArguments, prepare } from '../apps/native/prepare.mjs';

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

test('prepare writes portable app resources without an implicit test oracle', async t => {
  const temporary = await mkdtemp(join(tmpdir(), 'captures-native-'));
  t.after(() => rm(temporary, { recursive: true, force: true }));
  const output = join(temporary, 'resources');

  await prepare(output);

  assert.deepEqual((await readdir(output)).sort(), ['EDITOR-FONT-LICENSE.txt', 'dust.json', 'icon.svg', 'tokens.json']);
  assert.equal(await readFile(join(output, 'EDITOR-FONT-LICENSE.txt'), 'utf8'),
    await readFile(new URL('../crates/captures-app/fonts/liberation/LICENSE', import.meta.url), 'utf8'));
  const tokens = JSON.parse(await readFile(join(output, 'tokens.json'), 'utf8'));
  const dust = JSON.parse(await readFile(join(output, 'dust.json'), 'utf8'));
  assert.ok(tokens['light-cobalt']);
  assert.equal(dust.particles.length, 198);
  assert.equal((await readdir(temporary)).includes('poses.json'), false);
});

test('prepare writes poses only to an explicit test output', async t => {
  const temporary = await mkdtemp(join(tmpdir(), 'captures-native-'));
  t.after(() => rm(temporary, { recursive: true, force: true }));
  const output = join(temporary, 'resources');
  const testOutput = join(temporary, 'tests');

  await prepare(output, testOutput);

  const poses = JSON.parse(await readFile(join(testOutput, 'poses.json'), 'utf8'));
  assert.equal(poses.particles.length, 198);
  assert.equal(poses.times.length, poses.poses.length);
});

test('CLI arguments preserve defaults and select optional portable outputs', () => {
  const defaults = parseArguments([]);
  assert.ok(defaults.destination.endsWith(join('apps', 'native', 'macos', 'Sources', 'CapturesNative', 'Resources')));
  assert.ok(defaults.testDestination.endsWith(join('apps', 'native', 'macos', 'Tests', 'CapturesNativeTests', 'Resources')));

  assert.deepEqual(parseArguments(['--output', 'wgpu/resources']), {
    destination: resolve('wgpu/resources'), testDestination: undefined,
  });
  assert.deepEqual(parseArguments(['--test-output', 'oracle', '--output', 'resources']), {
    destination: resolve('resources'), testDestination: resolve('oracle'),
  });
});

test('CLI arguments reject unknown, missing and duplicate options', () => {
  assert.throws(() => parseArguments(['--wat', 'somewhere']), /Unknown argument/);
  assert.throws(() => parseArguments(['--output']), /Missing path/);
  assert.throws(() => parseArguments(['--output', '--test-output', 'somewhere']), /Missing path/);
  assert.throws(() => parseArguments(['--test-output', 'somewhere']), /--output is required/);
  assert.throws(() => parseArguments(['--output', 'one', '--output', 'two']), /Duplicate argument/);
});
