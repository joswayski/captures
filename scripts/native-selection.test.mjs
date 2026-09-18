import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import test from 'node:test';
import { dragSelectionRect, constrainSelectionToAspect } from '../apps/desktop/ui/src/lib/selection.ts';

const directory = new URL('../crates/captures-app/tests/', import.meta.url);
const fixture = new URL('selection-golden.json', directory);
const cases = [];
// Unequal dimensions, fractional coordinates, all corners, crossed anchors,
// out-of-bounds pointers, both aspect orientations, Shift precedence, tiny boxes.
for (const mode of ['create', 'move', 'nw', 'ne', 'sw', 'se']) {
  for (const aspectRatio of [null, 16 / 9, 9 / 16]) {
    for (const [origin, current] of [
      [{ x: 103.25, y: 71.75 }, { x: 421.5, y: 317.125 }],
      [{ x: 421.5, y: 317.125 }, { x: -17, y: -23 }],
      [{ x: 421.5, y: 71.75 }, { x: 2017, y: 2319 }],
      [{ x: 103.25, y: 317.125 }, { x: 103.25, y: 71.75 }],
    ]) {
      for (const forceSquare of [false, true]) {
        cases.push({ mode, origin, current,
          initial: { x: 103.25, y: 71.75, width: 318.25, height: 245.375 },
          bounds: { width: 823.5, height: 617.25 },
          options: { aspectRatio, forceSquare, minimumSize: 16 } });
      }
    }
  }
}
for (const mode of ['nw', 'ne', 'sw', 'se']) {
  for (const aspectRatio of [null, 1, 16 / 9, 9 / 16]) {
    for (const initial of [{ x: 1, y: 2, width: 3, height: 5 }, { x: 0, y: 0, width: 8, height: 7 }]) {
      cases.push({ mode, origin: { x: 1, y: 2 }, current: { x: 4, y: 6 }, initial,
        bounds: { width: 8, height: 7 }, options: { aspectRatio, forceSquare: false, minimumSize: 16 } });
    }
  }
}

function shippingCases() {
  return cases.map(c => ({ ...c,
    expected: dragSelectionRect(c.mode, c.origin, c.current, c.initial, c.bounds, c.options),
    constrained: constrainSelectionToAspect(c.initial, c.options.aspectRatio, c.bounds, c.options.minimumSize),
  }));
}

if (process.argv.includes('--write')) {
  await mkdir(directory, { recursive: true });
  // One complete case per line keeps generated provenance reviewable and bounded.
  await writeFile(fixture, `[\n${shippingCases().map(c => JSON.stringify(c)).join(',\n')}\n]\n`);
} else {
  test('native region vectors match the shipping TypeScript oracle', async () => {
    assert.deepEqual(JSON.parse(await readFile(fixture, 'utf8')), shippingCases());
  });
  test('vectors distinguish free and aspect-locked resize and Shift precedence', () => {
    const vectors = shippingCases();
    assert.equal(vectors.length, 176);
    const first = vectors[0];
    assert.notEqual(first.expected.width, first.expected.height);
    const square = vectors[1];
    assert.equal(square.expected.width, square.expected.height);
    assert.ok(vectors.some(v => v.mode === 'se' && v.expected.x < v.initial.x));
    assert.ok(vectors.some(v => v.expected.width < 16));
  });
}
