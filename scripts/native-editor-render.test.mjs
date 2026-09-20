import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import test from 'node:test';
import {
  closedShapePolygon,
  normalizeRect,
} from '../apps/desktop/ui/src/lib/screenshotEditor.ts';

const directory = new URL('../crates/captures-app/tests/', import.meta.url);
const fixture = new URL('editor-shape-golden.json', directory);
const kinds = ['rectangle', 'ellipse', 'triangle', 'diamond', 'star'];

function shippingCases() {
  return kinds.map((shape, index) => {
    const start = { x: 73.25 + index * 2.125, y: 42.25 - index * 1.75 };
    const end = { x: -12.75 + index * 0.625, y: -11.5 - index * 0.375 };
    const rect = normalizeRect(start, end);
    let variant = shape;
    let radius = null;
    let points = [];
    if (shape === 'rectangle') {
      variant = 'rounded-rectangle';
      radius = Math.min(12, rect.width / 6, rect.height / 6);
    } else if (shape === 'triangle' || shape === 'diamond' || shape === 'star') {
      variant = 'polygon';
      points = closedShapePolygon(shape, rect);
    }
    return {
      element: {
        id: `fixture-${shape}`,
        kind: 'shape',
        shape,
        x: start.x,
        y: start.y,
        endX: end.x,
        endY: end.y,
        controls: [],
        style: {
          color: '#1256aacc',
          fill: '#ef7139b3',
          strokeWidth: 3.25,
        },
        locked: index % 2 === 0,
        visible: true,
        opacity: 67,
        blendMode: 'screen',
      },
      expected: {
        variant,
        rect,
        radius,
        points,
      },
    };
  });
}

const serializableCases = () => JSON.parse(JSON.stringify(shippingCases()));

if (process.argv.includes('--write')) {
  await mkdir(directory, { recursive: true });
  await writeFile(fixture, `${JSON.stringify(serializableCases(), null, 2)}\n`);
} else {
  test('native closed-shape vectors match shipping TypeScript geometry', async () => {
    assert.deepEqual(JSON.parse(await readFile(fixture, 'utf8')), serializableCases());
  });

  test('closed-shape vectors cover reversed fractional geometry and the star ratio', () => {
    const cases = shippingCases();
    assert.deepEqual(cases.map(entry => entry.element.shape), kinds);
    assert.ok(cases.every(entry => entry.element.x > entry.element.endX));
    assert.ok(cases.every(entry => entry.element.y > entry.element.endY));
    const star = cases.find(entry => entry.element.shape === 'star');
    assert.equal(star.expected.points.length, 10);
    const centerX = star.expected.rect.x + star.expected.rect.width / 2;
    const outerX = star.expected.rect.width / 2;
    const inner = star.expected.points[1];
    assert.ok(Math.abs((inner.x - centerX) / (Math.cos(-Math.PI * 0.3) * outerX) - 0.39) < 1e-12);
  });
}
