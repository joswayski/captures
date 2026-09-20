import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import test from 'node:test';
import {
  boundedCropRect,
  createScreenshotDocument,
  cropDocument,
  resizeDocumentCanvas,
  transformImageElement,
  translateElement,
} from '../apps/desktop/ui/src/lib/screenshotEditor.ts';

const directory = new URL('../crates/captures-app/tests/', import.meta.url);
const fixture = new URL('editor-document-golden.json', directory);

const base = {
  locked: false,
  visible: true,
  opacity: 73,
  blendMode: 'multiply',
};
const style = {
  color: '#123456',
  fill: null,
  strokeWidth: 3.5,
  futureStyle: { keep: ['nested', 17] },
};
const document = {
  width: 713,
  height: 257,
  background: null,
  futureDocument: { version: 9, keep: true },
  elements: [
    {
      ...base,
      id: 'hidden-image',
      kind: 'image',
      source: 'imported',
      src: 'draft-asset:image',
      originalSrc: null,
      name: 'image.png',
      sourceArtifactId: 'artifact-image',
      x: -27.25,
      y: 14.75,
      width: 91.5,
      height: 43.25,
      naturalWidth: 183,
      naturalHeight: 86.5,
      visible: false,
      futureImage: { alpha: 0.25 },
    },
    {
      ...base,
      id: 'locked-text',
      kind: 'text',
      x: 511.125,
      y: -19.5,
      rotation: 0.375,
      locked: true,
      text: 'asymmetric',
      fontSize: 17,
      width: 143.5,
      fontFamily: 'rounded',
      bold: true,
      italic: false,
      align: 'right',
      color: '#abcdef',
      background: null,
      outlined: false,
      roundedBackground: true,
      futureText: ['preserve'],
    },
    {
      ...base,
      id: 'shape',
      kind: 'shape',
      shape: 'arrow',
      x: 33.25,
      y: 44.5,
      endX: 201.75,
      endY: -8.125,
      controls: [{ x: 71.5, y: -12.25 }, { x: 149.125, y: 88.75 }],
      style,
      futureShape: 'keep',
    },
    {
      ...base,
      id: 'path',
      kind: 'path',
      x: -61.5,
      y: 199.25,
      points: [{ x: -61.5, y: 199.25 }, { x: 0.125, y: 257.875 }],
      style: { ...style, dropShadow: true },
      futurePath: 42,
    },
  ],
};

function geometryCases() {
  const bounds = { width: 713, height: 257 };
  const crops = [
    [{ x: -31.25, y: 19.5 }, { x: 801.75, y: 300.125 }, null],
    [{ x: 688.75, y: 241.25 }, { x: -44.5, y: -90.75 }, null],
    [{ x: 611.125, y: 17.75 }, { x: -55.5, y: 249.875 }, 16 / 9],
    [{ x: 101.375, y: 233.5 }, { x: 699.75, y: -18.25 }, 9 / 16],
    [{ x: 300.5, y: 120.5 }, { x: 300.5, y: 120.5 }, 1],
  ].map(([start, end, aspectRatio]) => ({
    start,
    end,
    bounds,
    aspectRatio,
    expected: boundedCropRect(start, end, bounds, aspectRatio),
  }));
  const translations = [
    { deltaX: -103.75, deltaY: 29.125 },
    { deltaX: 801.5, deltaY: -411.25 },
  ].map(offset => ({
    ...offset,
    expected: {
      ...document,
      elements: document.elements.map(element => translateElement(element, offset.deltaX, offset.deltaY)),
    },
  }));
  const cropRects = [
    { x: 71.6, y: 19.49, width: 580.51, height: 190.5 },
    { x: -300.25, y: 251.6, width: 900.75, height: 100.25 },
  ].map(rect => ({ rect, expected: cropDocument(document, rect) }));
  const canvasSizes = [
    { width: 100.49, height: 200.5 },
    { width: -7.25, height: 40_000.75 },
  ].map(size => ({ ...size, expected: resizeDocumentCanvas(document, size.width, size.height) }));
  return { crops, translations, cropRects, canvasSizes };
}

function orientationCases() {
  const orientations = [
    undefined,
    'rotate-90',
    'rotate-180',
    'rotate-270',
    'flip-horizontal',
    'flip-vertical',
    'transpose',
    'transverse',
  ];
  const actions = [
    'rotate-clockwise',
    'rotate-counterclockwise',
    'flip-horizontal',
    'flip-vertical',
  ];
  return orientations.flatMap(orientation => actions.map(action => {
    const input = {
      ...document.elements[0],
      visible: true,
      x: -17.25,
      y: 31.75,
      width: 83.5,
      height: 42.25,
      ...(orientation ? { orientation } : {}),
    };
    return { input, action, expected: transformImageElement(input, action) };
  }));
}

function historyCases() {
  const initial = createScreenshotDocument('draft-asset:background', 19.5, 11.25, 'capture-1');
  const operations = [{ kind: 'commit', width: initial.width }];
  for (let i = 1; i <= 102; i += 1) {
    operations.push({ kind: 'commit', width: i, marker: i });
  }
  operations.push({ kind: 'undo' }, { kind: 'undo' }, { kind: 'redo' });
  operations.push({ kind: 'commit', width: 777, branch: true });
  operations.push({ kind: 'redo' });
  for (let i = 0; i < 105; i += 1) operations.push({ kind: 'undo' });
  for (let i = 0; i < 105; i += 1) operations.push({ kind: 'redo' });

  let current = initial;
  let undo = [];
  let redo = [];
  const expected = [];
  for (const operation of operations) {
    let changed = false;
    if (operation.kind === 'commit') {
      const next = {
        ...initial,
        width: operation.width,
        ...(operation.marker === undefined ? {} : { marker: { i: operation.marker } }),
        ...(operation.branch ? { branch: true } : {}),
      };
      if (JSON.stringify(current) !== JSON.stringify(next)) {
        undo = [...undo.slice(-99), current];
        redo = [];
        current = next;
        changed = true;
      }
    } else if (operation.kind === 'undo' && undo.length > 0) {
      const previous = undo.at(-1);
      redo = [current, ...redo].slice(0, 100);
      current = previous;
      undo = undo.slice(0, -1);
      changed = true;
    } else if (operation.kind === 'redo' && redo.length > 0) {
      const next = redo[0];
      undo = [...undo.slice(-99), current];
      current = next;
      redo = redo.slice(1);
      changed = true;
    }
    expected.push({
      changed,
      current: {
        width: current.width,
        marker: current.marker?.i ?? null,
        branch: current.branch === true,
      },
      undo: undo.length,
      redo: redo.length,
    });
  }
  return { initial, operations, expected };
}

function shippingCases() {
  return {
    initialization: {
      input: { src: 'draft-asset:background', width: 19.5, height: 11.25, sourceArtifactId: 'capture-1' },
      expected: createScreenshotDocument('draft-asset:background', 19.5, 11.25, 'capture-1'),
    },
    document,
    ...geometryCases(),
    orientations: orientationCases(),
    history: historyCases(),
  };
}

function serializableCases() {
  return JSON.parse(JSON.stringify(shippingCases()));
}

function fixtureText(cases) {
  const array = (values, indent) => [
    '[',
    ...values.map((value, index) => `${indent}${JSON.stringify(value)}${index + 1 < values.length ? ',' : ''}`),
    `${indent.slice(2)}]`,
  ].join('\n');
  return [
    '{',
    `  "initialization": ${JSON.stringify(cases.initialization)},`,
    `  "document": ${JSON.stringify(cases.document)},`,
    `  "crops": ${array(cases.crops, '    ')},`,
    `  "translations": ${array(cases.translations, '    ')},`,
    `  "cropRects": ${array(cases.cropRects, '    ')},`,
    `  "canvasSizes": ${array(cases.canvasSizes, '    ')},`,
    `  "orientations": ${array(cases.orientations, '    ')},`,
    '  "history": {',
    `    "initial": ${JSON.stringify(cases.history.initial)},`,
    `    "operations": ${array(cases.history.operations, '      ')},`,
    `    "expected": ${array(cases.history.expected, '      ')}`,
    '  }',
    '}',
    '',
  ].join('\n');
}

if (process.argv.includes('--write')) {
  await mkdir(directory, { recursive: true });
  await writeFile(fixture, fixtureText(serializableCases()));
} else {
  test('native editor vectors match the shipping TypeScript oracle', async () => {
    assert.deepEqual(JSON.parse(await readFile(fixture, 'utf8')), serializableCases());
  });
  test('vectors cover geometry, every D4 state, unknown data, and history boundaries', () => {
    const vectors = shippingCases();
    assert.equal(vectors.orientations.length, 32);
    assert.ok(vectors.document.elements.some(element => element.visible === false));
    assert.ok(vectors.document.elements.some(element => element.locked === true));
    assert.ok(vectors.crops.some(entry => entry.start.x < 0));
    assert.ok(vectors.history.expected.some(entry => entry.undo === 100));
    assert.ok(vectors.history.expected.some(entry => entry.redo === 100));
    assert.equal(vectors.history.expected[0].changed, false);
  });
}
