import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import test from 'node:test';
import { windowPointerHoverAtPoint } from '../apps/desktop/ui/src/lib/selection.ts';

const fixture = new URL('../crates/captures-app/tests/window-hit-golden.json', import.meta.url);
const window = (id, z_order, x, y, width, height) => ({
  id, z_order, x, y, width, height, title: id, app_name: 'Fixture', display_id: 'left',
});

function shippingCases() {
  const cases = [];
  const origin = { x: -320, y: 37 };
  for (const tied of [false, true]) {
    // Array order deliberately disagrees with z-order. A zero-width window
    // must not steal hits, and an equal-level shell follows the window list.
    const windows = [
      window('rear', -9, -320, 37, 301, 173),
      window('front', 4, -280, 61, 91, 67),
      window('neighbor', 2, -189, 61, 113, 67),
      window('tied', 4, -280, 61, 91, 67),
      window('empty', 99, -280, 61, 0, 100),
    ];
    const shell = [window('panel', tied ? 4 : 10, -320, 37, 301, 30)];
    for (const scale of [-1, 0, 1, 1.25, 2]) {
      const safeScale = scale > 0 ? scale : 1;
      for (const [x, y] of [[0, 0], [40, 24], [41, 29.99], [41, 30], [130.99, 40],
        [131, 40], [80, 90.99], [80, 91], [244, 40], [300.99, 172.99], [301, 173], [-.01, 40]]) {
        const point = { x: x / safeScale, y: y / safeScale };
        const hit = windowPointerHoverAtPoint(windows, shell, point, origin, scale, true);
        cases.push({ windows, shell, origin, scale, point,
          expected: hit.windowId === null ? null : windows.findIndex(w => w.id === hit.windowId) });
      }
    }
  }
  return cases;
}

if (process.argv.includes('--write')) {
  await writeFile(fixture, `[\n${shippingCases().map(c => JSON.stringify(c)).join(',\n')}\n]\n`);
} else {
  test('native window hit vectors match the shipping TypeScript oracle', async () => {
    assert.deepEqual(JSON.parse(await readFile(fixture, 'utf8')), shippingCases());
  });
  test('hit vectors discriminate z-order, shell ties, and shared half-open edges', () => {
    const cases = shippingCases();
    assert.equal(cases.length, 120);
    assert.equal(cases[1].expected, null); // Higher panel beats the front window.
    assert.equal(cases[61].expected, 1); // Equal panel loses to the stable window list.
    assert.equal(cases[4].expected, 1); // Just inside the front right edge.
    assert.equal(cases[5].expected, 2); // Exact edge belongs to its neighbor.
    assert.equal(cases[7].expected, 0); // Rear window below the front bottom edge.
    assert.equal(cases[10].expected, null); // Exact display bounds are outside.
  });
}
