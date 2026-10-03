import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { declarations, resolveTokens, color, easing, boxShadow, componentShadows, themes, particleFixture, parseArguments, prepare } from '../apps/native/prepare.mjs';

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

test('native tokens carry every ease token as cubic-bezier control points', async t => {
  assert.deepEqual(easing('cubic-bezier(0.16, 1, 0.3, 1)'), [0.16, 1, 0.3, 1]);
  assert.equal(easing('#ffffff'), null);
  assert.throws(() => easing('cubic-bezier(1.5, 0, 0, 1)'), /Invalid easing/);
  assert.throws(() => easing('cubic-bezier(0, 0, 1)'), /Invalid easing/);
  const temporary = await mkdtemp(join(tmpdir(), 'captures-native-'));
  t.after(() => rm(temporary, { recursive: true, force: true }));
  await prepare(temporary);
  const tokens = JSON.parse(await readFile(join(temporary, 'tokens.json'), 'utf8'));
  for (const variant of Object.values(tokens)) {
    assert.deepEqual(variant.easings['ease-out'], [0.16, 1, 0.3, 1]);
    assert.deepEqual(variant.easings['ease-standard'], [0.2, 0.8, 0.2, 1]);
    assert.deepEqual(variant.easings['ease-in'], [0.4, 0, 1, 1]);
    assert.deepEqual(variant.easings['ease-in-out'], [0.45, 0, 0.55, 1]);
    assert.equal(variant.numbers['dur-4'], 280);
  }
});

test('native tokens carry box-shadow layers, including the preview card shadow', async t => {
  assert.deepEqual(boxShadow('0 16px 44px rgba(0, 0, 0, 0.44), 0 2px 8px rgba(0, 0, 0, 0.3)'), [
    { x: 0, y: 16, blur: 44, spread: 0, color: [0, 0, 0, 0.44] },
    { x: 0, y: 2, blur: 8, spread: 0, color: [0, 0, 0, 0.3] },
  ]);
  assert.equal(boxShadow('drop-shadow(0 4px 10px rgba(0, 0, 0, 0.32))'), null);
  assert.throws(() => boxShadow('inset 0 1px 2px rgba(0, 0, 0, 0.3)'), /Unsupported box-shadow/);
  assert.throws(() => boxShadow('0 1px 2px rgba(0, 0, 0, 0.3), inset 0 0 1px #ffffff'), /Unsupported box-shadow/);
  assert.throws(() => componentShadows('.other { --x: 1; }'), /Missing/);
  const temporary = await mkdtemp(join(tmpdir(), 'captures-native-'));
  t.after(() => rm(temporary, { recursive: true, force: true }));
  await prepare(temporary);
  const tokens = JSON.parse(await readFile(join(temporary, 'tokens.json'), 'utf8'));
  for (const variant of Object.values(tokens)) {
    assert.deepEqual(variant.shadows['thumbnail-card-shadow'], [
      { x: 0, y: 6, blur: 14, spread: 0, color: [0, 0, 0, 0.38] },
      { x: 0, y: 2, blur: 5, spread: 0, color: [0, 0, 0, 0.26] },
    ]);
    assert.equal(variant.shadows['glass-shadow'].length, 2);
    assert.equal(variant.shadows['tooltip-shadow'], undefined);
  }
  assert.deepEqual(tokens['dark-cobalt'].shadows['shadow-sm'], [{ x: 0, y: 2, blur: 6, spread: 0, color: [0, 0, 0, 0.32] }]);
  assert.equal(tokens['light-cobalt'].shadows['shadow-sm'].length, 2);
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

  assert.deepEqual((await readdir(output)).sort(), ['EDITOR-FONT-LICENSE.txt', 'dust.json', 'icon.svg', 'tokens.json', 'tray-source.png']);
  assert.deepEqual(await readFile(join(output, 'tray-source.png')),
    await readFile(new URL('../apps/desktop/src-tauri/icons/icon.png', import.meta.url)));
  assert.equal(await readFile(join(output, 'EDITOR-FONT-LICENSE.txt'), 'utf8'),
    `${await readFile(new URL('../crates/captures-app/fonts/liberation/LICENSE', import.meta.url), 'utf8')}\n\n${await readFile(new URL('../crates/captures-app/fonts/nunito/OFL.txt', import.meta.url), 'utf8')}`);
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
