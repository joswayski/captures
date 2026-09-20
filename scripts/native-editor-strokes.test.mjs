import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import test from 'node:test';
import {
  arrowFillPolygon,
  elementRotationOrigin,
  sampleArrowPath,
} from '../apps/desktop/ui/src/lib/screenshotEditor.ts';

const directory = new URL('../crates/captures-app/tests/', import.meta.url);
const fixture = new URL('editor-stroke-golden.json', directory);

const style = {
  color: '#2b71c9d9',
  fill: null,
  strokeWidth: 7.25,
};

function element(shape, suffix, controls, rotation) {
  return {
    id: `${shape}-${suffix}`,
    kind: 'shape',
    shape,
    x: -13.25,
    y: 17.75,
    endX: 91.5,
    endY: 63.125,
    controls,
    style,
    locked: suffix === 'multi',
    visible: true,
    opacity: 61,
    blendMode: 'overlay',
    rotation,
  };
}

function summarize(points, extraIndexes = []) {
  if (points.length === 0) {
    return { count: 0, checkpoints: [], bounds: null };
  }
  const indexes = [...new Set([
    0,
    1,
    Math.floor(points.length / 4),
    Math.floor(points.length / 2),
    Math.floor(points.length * 3 / 4),
    points.length - 2,
    points.length - 1,
    ...extraIndexes,
  ].filter(index => index >= 0 && index < points.length))];
  const xs = points.map(point => point.x);
  const ys = points.map(point => point.y);
  return {
    count: points.length,
    checkpoints: indexes.map(index => ({ index, point: points[index] })),
    bounds: {
      x: Math.min(...xs),
      y: Math.min(...ys),
      width: Math.max(...xs) - Math.min(...xs),
      height: Math.max(...ys) - Math.min(...ys),
    },
  };
}

function shippingCases() {
  const controls = [
    ['straight', [], -0.375],
    ['single', [{ x: 18.625, y: -29.5 }], 0.625],
    ['multi', [
      { x: 4.5, y: 88.25 },
      { x: 42.75, y: -17.125 },
      { x: 73.375, y: 94.5 },
    ], -1.125],
  ];
  return ['line', 'arrow'].flatMap(shape => controls.map(([suffix, points, rotation]) => {
    const current = element(shape, suffix, points, rotation);
    const polygon = arrowFillPolygon(current);
    const tipIndex = polygon.findIndex(point => (
      point.x === current.endX && point.y === current.endY
    ));
    return {
      element: current,
      expected: {
        samples48: summarize(sampleArrowPath(current, 48)),
        polygon: summarize(polygon, [tipIndex]),
        rotationOrigin: elementRotationOrigin(current),
      },
    };
  }));
}

const serializableCases = () => JSON.parse(JSON.stringify(shippingCases()));

if (process.argv.includes('--write')) {
  await mkdir(directory, { recursive: true });
  await writeFile(fixture, `${JSON.stringify(serializableCases(), null, 2)}\n`);
} else {
  test('native open-stroke vectors match shipping TypeScript geometry', async () => {
    assert.deepEqual(JSON.parse(await readFile(fixture, 'utf8')), serializableCases());
  });

  test('stroke vectors distinguish straight, quadratic, multi-control, and tapered arrows', () => {
    const cases = shippingCases();
    assert.equal(cases.length, 6);
    assert.equal(cases.find(entry => entry.element.id === 'line-straight').expected.samples48.count, 49);
    assert.equal(cases.find(entry => entry.element.id === 'line-single').expected.samples48.count, 49);
    assert.equal(cases.find(entry => entry.element.id === 'line-multi').expected.samples48.count, 145);
    for (const entry of cases.filter(entry => entry.element.shape === 'line')) {
      assert.equal(entry.expected.polygon.count, 0);
    }
    for (const entry of cases.filter(entry => entry.element.shape === 'arrow')) {
      assert.ok(entry.expected.polygon.count > 50);
      assert.ok(entry.expected.polygon.checkpoints.some(({ point }) => (
        point.x === entry.element.endX && point.y === entry.element.endY
      )));
    }
  });
}
