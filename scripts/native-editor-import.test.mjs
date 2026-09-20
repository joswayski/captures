import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import test from 'node:test';
import {
  elementBounds,
  expandDocumentForElement,
  imageDropExpandPadding,
  imageDropPlacementAtPoint,
  isFullyOutsideCanvas,
  positionImportedImageAtEdge,
  resolveImageDropTarget,
} from '../apps/desktop/ui/src/lib/screenshotEditor.ts';

const directory = new URL('../crates/captures-app/tests/', import.meta.url);
const fixture = new URL('editor-import-golden.json', directory);

const image = (id, x, y, width, height, options = {}) => ({
  id,
  kind: 'image',
  source: 'imported',
  src: `fixture:${id}`,
  originalSrc: null,
  name: `${id}.png`,
  sourceArtifactId: null,
  x,
  y,
  width,
  height,
  naturalWidth: width,
  naturalHeight: height,
  locked: options.locked ?? false,
  visible: options.visible ?? true,
  opacity: 100,
  blendMode: 'source-over',
  ...(options.rotation === undefined ? {} : { rotation: options.rotation }),
});

function inputs() {
  return [
    {
      name: 'top-edge-preserves-sub-half-floating-point-rounding',
      document: {
        width: 7,
        height: 5,
        background: null,
        elements: [image('sub-half-target', 0.49999999999999994, 2, 3, 2)],
      },
      selectedId: null,
      point: { x: 1, y: 1.9 },
      natural: { width: 3, height: 1 },
    },
    {
      name: 'negative-half-ties-round-toward-positive-infinity',
      document: {
        width: 9,
        height: 7,
        background: null,
        elements: [image('negative-half-target', -2, -3, 6, 5)],
      },
      selectedId: null,
      point: { x: 1, y: -1 },
      natural: { width: 3, height: 1 },
    },
    {
      name: 'right-edge-resolves-rotated-visible-target-bounds',
      document: {
        width: 29,
        height: 23,
        background: null,
        elements: [
          image('hidden-front', 9, 3, 13, 11, { visible: false }),
          image('rotated-locked', 5, 4, 8, 4, {
            locked: true,
            rotation: Math.PI / 2,
          }),
        ],
      },
      selectedId: 'hidden-front',
      point: { x: 11.1, y: 6 },
      natural: { width: 5, height: 3 },
    },
    {
      name: 'negative-left-edge-natural-size',
      document: {
        width: 23,
        height: 17,
        background: null,
        elements: [image('negative-locked', -7.25, 3.5, 9, 5, { locked: true })],
      },
      selectedId: null,
      point: { x: -8.5, y: 5.25 },
      natural: { width: 6, height: 3 },
    },
    {
      name: 'stack-caps-asymmetric-overlay-on-top-visible-target',
      document: {
        width: 230,
        height: 170,
        background: null,
        elements: [
          image('background', 0, 0, 230, 170),
          image('hidden-front', 67, 41, 101, 83, { visible: false }),
          image('locked-overlay', 44.25, 27.5, 111, 89, { locked: true }),
        ],
      },
      selectedId: 'background',
      point: { x: 91.75, y: 73.25 },
      natural: { width: 401, height: 131 },
    },
    {
      name: 'outside-pointer-uses-frontmost-equidistant-visible-target',
      document: {
        width: 80,
        height: 40,
        background: null,
        elements: [
          image('left', -4.5, 8.25, 20, 11),
          image('right', 44.5, 8.25, 20, 11, { locked: true }),
          image('hidden', 21, 4, 18, 19, { visible: false }),
        ],
      },
      selectedId: 'hidden',
      point: { x: 30, y: 13.75 },
      natural: { width: 7, height: 4 },
    },
    {
      name: 'missing-selection-defaults-below-frontmost-visible-image',
      document: {
        width: 37,
        height: 29,
        background: null,
        elements: [
          image('visible-back', 1.25, 2.5, 13, 9),
          image('hidden-selected', -19, -17, 5, 3, { visible: false, locked: true }),
          image('visible-front', 18.5, 11.25, 7, 5, { locked: true }),
        ],
      },
      selectedId: 'hidden-selected',
      point: null,
      natural: { width: 9, height: 4 },
    },
    {
      name: 'fully-outside-stack-expands-with-padding-and-translates-all-layers',
      document: {
        width: 31,
        height: 19,
        background: null,
        elements: [
          image('on-canvas-hidden', 2.25, 1.5, 8, 6, { visible: false, locked: true }),
          image('outside-target', -29.5, -14.25, 11, 7, { locked: true }),
        ],
      },
      selectedId: null,
      point: { x: -24, y: -10.75 },
      natural: { width: 5, height: 3 },
    },
  ];
}

function shippingCase(input) {
  const target = resolveImageDropTarget(input.document, input.selectedId, input.point ?? undefined);
  const point = input.point ?? {
    x: target.x + target.width / 2,
    y: target.y + target.height,
  };
  const placement = input.point
    ? imageDropPlacementAtPoint(point, target)
    : 'bottom';
  const position = positionImportedImageAtEdge(
    input.natural.width,
    input.natural.height,
    input.document,
    target,
    placement,
    point,
  );
  const imported = {
    ...image('new-import', position.x, position.y, position.width, position.height),
    naturalWidth: input.natural.width,
    naturalHeight: input.natural.height,
  };
  const fullyOutside = isFullyOutsideCanvas(elementBounds(imported), input.document);
  const output = fullyOutside
    ? expandDocumentForElement(
      input.document,
      imported,
      imageDropExpandPadding(placement),
    )
    : { ...input.document, elements: [...input.document.elements, imported] };
  return {
    ...input,
    expected: {
      target,
      placement,
      position,
      fullyOutside,
      output: {
        width: output.width,
        height: output.height,
        elements: output.elements.map(({ id, x, y, width, height }) => ({
          id, x, y, width, height,
        })),
      },
    },
  };
}

const shippingCases = () => inputs().map(shippingCase);
const serializableCases = () => JSON.parse(JSON.stringify(shippingCases()));

if (process.argv.includes('--write')) {
  await mkdir(directory, { recursive: true });
  await writeFile(fixture, `${JSON.stringify(serializableCases(), null, 2)}\n`);
} else {
  test('native image-import vectors match shipping TypeScript placement', async () => {
    assert.deepEqual(JSON.parse(await readFile(fixture, 'utf8')), serializableCases());
  });

  test('image-import vectors discriminate target, sizing, and expansion policies', () => {
    const cases = shippingCases();
    const subHalf = cases.find(entry => entry.name.startsWith('top-edge'));
    assert.equal(subHalf.expected.placement, 'top');
    assert.equal(subHalf.expected.position.x, 0);
    const negativeHalf = cases.find(entry => entry.name.startsWith('negative-half'));
    assert.ok(Object.is(negativeHalf.expected.position.x, -0));
    assert.equal(negativeHalf.expected.position.y, -1);
    const rotated = cases.find(entry => entry.name.startsWith('right-edge'));
    assert.equal(rotated.expected.placement, 'right');
    assert.notEqual(rotated.expected.target.width, rotated.document.elements[1].width);
    assert.ok(cases.some(entry => entry.expected.position.x < 0));
    assert.ok(cases.some(entry => entry.expected.placement === 'stack'));
    assert.ok(cases.some(entry => entry.expected.fullyOutside));
    const capped = cases.find(entry => entry.name.startsWith('stack-caps'));
    assert.ok(capped.expected.position.width < capped.natural.width);
    const natural = cases.find(entry => entry.name.startsWith('negative-left'));
    assert.equal(natural.expected.position.width, natural.natural.width);
  });
}
