import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import test from 'node:test';
import {
  ARROW_MIN_DRAW_LENGTH,
  arrowPathLength,
  boundedCropRect,
  createScreenshotDocument,
  cropDragAspectRatio,
  cropDocument,
  duplicateScreenshotElement,
  elementBounds,
  elementLocalBounds,
  elementWorldPoint,
  expandDocumentToFitBounds,
  hitTestElement,
  isFullyOutsideCanvas,
  reorderScreenshotLayers,
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
  return { crops, cropDrags: cropDragCases(), translations, cropRects, canvasSizes };
}

function cropDragCase(origin, bounds, initialPreset, initialShiftKey, updates) {
  let shiftAspect = null;
  let liveRect = null;
  const step = ({ current, preset = 'free', presetAspect = null, shiftKey = false }) => {
    const next = cropDragAspectRatio({
      preset,
      shiftKey,
      origin,
      current,
      bounds,
      shiftAspect,
      liveRect,
    });
    liveRect = boundedCropRect(origin, current, bounds, next.aspectRatio);
    shiftAspect = next.shiftAspect;
    return { current, presetAspect, shiftKey, expected: liveRect };
  };
  const initial = step({
    current: origin,
    preset: initialPreset.preset,
    presetAspect: initialPreset.aspect,
    shiftKey: initialShiftKey,
  });
  return {
    origin,
    bounds,
    initial,
    updates: updates.map(step),
  };
}

function cropDragCases() {
  return [
    cropDragCase(
      { x: -31.25, y: 300.5 },
      { width: 713, height: 257 },
      { preset: 'free', aspect: null },
      false,
      [
        { current: { x: 801.75, y: 19.5 } },
        { current: { x: 113.49, y: 99.5 } },
      ],
    ),
    ...[
      [{ x: 611.125, y: 17.75 }, { x: -55.5, y: 249.875 }],
      [{ x: 101.375, y: 233.5 }, { x: 699.75, y: -18.25 }],
      [{ x: 611.125, y: 233.5 }, { x: -55.5, y: -18.25 }],
      [{ x: 101.375, y: 17.75 }, { x: 799.75, y: 249.875 }],
    ].map(([origin, current]) => cropDragCase(
      origin,
      { width: 713, height: 257 },
      { preset: '16:9', aspect: 16 / 9 },
      true,
      [{ current, preset: '16:9', presetAspect: 16 / 9, shiftKey: true }],
    )),
    cropDragCase(
      { x: 50, y: 50 },
      { width: 1_000, height: 800 },
      { preset: 'free', aspect: null },
      false,
      [
        { current: { x: 250, y: 150 } },
        { current: { x: 400, y: 600 }, shiftKey: true },
        { current: { x: 120, y: 240 }, shiftKey: true },
        { current: { x: 170, y: 90 }, shiftKey: false },
        { current: { x: 400, y: 600 }, shiftKey: true },
        { current: { x: 900, y: 300 }, preset: '16:9', presetAspect: 16 / 9, shiftKey: true },
        { current: { x: 300, y: 700 }, shiftKey: true },
      ],
    ),
    cropDragCase(
      { x: 300.5, y: 120.5 },
      { width: 713, height: 257 },
      { preset: 'free', aspect: null },
      true,
      [
        { current: { x: 304.25, y: 123.75 }, shiftKey: true },
        { current: { x: 290.25, y: 131.75 }, shiftKey: true },
        { current: { x: 304.25, y: 123.75 }, shiftKey: false },
        { current: { x: 500.25, y: 220.75 }, shiftKey: true },
      ],
    ),
  ];
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
  const elements = [...document.elements, { ...document.elements[0], id: 'above', visible: true }];
  const layers = {
    elements,
    reorders: elements.flatMap(moved => elements.flatMap(target => ['before', 'after'].map(placement => ({
      moved: moved.id,
      target: target.id,
      placement,
      expected: reorderScreenshotLayers(elements, moved.id, target.id, placement).map(element => element.id),
    })))),
    duplicates: document.elements.map(input => ({
      input,
      expected: duplicateScreenshotElement(input, `${input.id}-copy`),
    })),
  };
  return {
    initialization: {
      input: { src: 'draft-asset:background', width: 19.5, height: 11.25, sourceArtifactId: 'capture-1' },
      expected: createScreenshotDocument('draft-asset:background', 19.5, 11.25, 'capture-1'),
    },
    document,
    ...geometryCases(),
    shapeCreations: shapeCreationCases(),
    openShapeCreations: openShapeCreationCases(),
    freehandCreations: freehandCreationCases(),
    hitTests: hitTestCases(),
    orientations: orientationCases(),
    layers,
    history: historyCases(),
  };
}

function hitTestCases() {
  const image = { ...document.elements[0], id: 'image', visible: true };
  const shape = document.elements[2];
  const path = document.elements[3];
  const variants = [
    image,
    { ...image, rotation: 0.67, orientation: 'rotate-90' },
    ...['rectangle', 'ellipse'].map(kind => ({
      ...shape, shape: kind, rotation: -0.39,
      style: { ...style, strokeEnabled: false, dropShadow: true },
    })),
    ...['line', 'arrow'].flatMap(kind => [
      { ...shape, shape: kind, controls: [] },
      { ...shape, shape: kind, controls: [{ x: 21.25, y: 279.75 }], rotation: 0.41 },
      { ...shape, shape: kind, rotation: -0.79, style: { ...style, dropShadow: true } },
    ]),
    { ...shape, endX: shape.x, endY: shape.y, controls: [] },
    { ...path, points: [] },
    { ...path, points: [path.points[0]], rotation: 0.33 },
    { ...path, rotation: -0.51 },
  ];
  const scenario = (name, elements, queries, bounds = null) => ({
    name, input: { ...document, elements }, bounds,
    outline: bounds ? [
      { x: bounds.x, y: bounds.y },
      { x: bounds.x + bounds.width, y: bounds.y },
      { x: bounds.x + bounds.width, y: bounds.y + bounds.height },
      { x: bounds.x, y: bounds.y + bounds.height },
    ].map(point => elementWorldPoint(elements[0], point)) : null,
    queries: queries.map(({ point, tolerance }) => ({
      point, tolerance, expected: hitTestElement(elements, point, tolerance)?.id ?? null,
    })),
  });
  const cases = variants.map((element, index) => {
    const bounds = elementLocalBounds(element);
    // Both sides of every local edge, with and without document-space tolerance.
    // Rotation distinguishes local boxes from world-axis-aligned bounding boxes.
    const queries = [0, 8].flatMap(tolerance => [-0.125, 0.125].flatMap(offset => [
      { x: bounds.x - tolerance + offset, y: bounds.y + bounds.height * 0.37 },
      { x: bounds.x + bounds.width + tolerance + offset, y: bounds.y + bounds.height * 0.61 },
      { x: bounds.x + bounds.width * 0.23, y: bounds.y - tolerance + offset },
      { x: bounds.x + bounds.width * 0.71, y: bounds.y + bounds.height + tolerance + offset },
    ].map(local => ({ point: elementWorldPoint(element, local), tolerance }))));
    return scenario(`geometry-${index}-${element.kind}`, [element], queries, bounds);
  });
  const center = { x: image.x + image.width / 2, y: image.y + image.height / 2 };
  const queries = [{ point: center, tolerance: 0 }];
  cases.push(
    scenario('frontmost', [image, { ...image, id: 'front' }], queries),
    scenario('hidden-locked', [image, { ...image, id: 'locked', locked: true }, { ...image, id: 'hidden', visible: false }], queries),
    scenario('transparent-still-selectable', [image, { ...image, id: 'transparent', opacity: 0 }], queries),
    scenario('miss-front-hit-back', [image, { ...image, id: 'front', x: 999 }], queries),
    scenario('empty', [], queries),
    scenario('inclusive-image-edge', [image], [
      { point: { x: image.x - 8, y: center.y }, tolerance: 8 },
      { point: { x: image.x + image.width, y: image.y + image.height }, tolerance: 0 },
    ]),
    // A needless inverse rotation at zero loses precision on this exact edge.
    scenario('fractional-unrotated-edge', [{ ...image, x: 0.3 }], [
      { point: { x: 0.3, y: center.y }, tolerance: 0 },
    ]),
  );
  return cases;
}

function shapeCreationCases() {
  const input = {
    width: 40,
    height: 30,
    background: '#ffffff',
    futureDocument: { preserve: 'creation' },
    elements: [{
      id: 'existing-hidden-locked',
      kind: 'image',
      source: 'imported',
      src: 'draft-asset:existing',
      originalSrc: null,
      name: 'existing.png',
      sourceArtifactId: null,
      x: 1.25,
      y: -2.5,
      width: 3,
      height: 2,
      naturalWidth: 3,
      naturalHeight: 2,
      locked: true,
      visible: false,
      opacity: 63,
      blendMode: 'screen',
      futureImage: ['keep'],
    }],
  };
  const defaults = {
    color: '#ff3b5c',
    fill: '#ff3b5c',
    strokeWidth: 8,
    strokeEnabled: false,
    dropShadow: false,
  };
  const vectors = [
    {
      name: 'reverse fractional rectangle remains clipped when partially overlapping',
      shape: 'rectangle',
      start: { x: 2.25, y: 15.5 },
      end: { x: -3.75, y: 4.125 },
    },
    {
      name: 'negative fully outside ellipse expands and translates every layer',
      shape: 'ellipse',
      start: { x: -30.25, y: -20.5 },
      end: { x: -18.75, y: -12.25 },
    },
    {
      name: 'right bottom fully outside rectangle expands without translation',
      shape: 'rectangle',
      start: { x: 46.25, y: 34.5 },
      end: { x: 54.75, y: 41.25 },
    },
    {
      name: 'custom ellipse retains style opacity and unknown style data',
      shape: 'ellipse',
      start: { x: 31.75, y: 3.25 },
      end: { x: 17.125, y: 22.875 },
      style: {
        color: '#00ff00',
        fill: null,
        strokeWidth: 2.5,
        strokeEnabled: true,
        dropShadow: false,
        futureStyle: { preserve: 7 },
      },
      opacity: 37.5,
    },
    {
      name: 'default shadow padding keeps shadow-only canvas overlap clipped',
      shape: 'rectangle',
      start: { x: -25, y: 9.5 },
      end: { x: -21, y: 14.5 },
      style: { ...defaults, dropShadow: true },
    },
    {
      name: 'custom shadow padding expands fully outside shape and translates siblings',
      shape: 'ellipse',
      start: { x: -50, y: -31.25 },
      end: { x: -46, y: -26.75 },
      style: {
        color: '#5533cc',
        fill: '#5533cc',
        strokeWidth: 2,
        strokeEnabled: false,
        dropShadow: true,
        dropShadowStyle: {
          color: '#112233',
          opacity: 80,
          blur: 4,
          offsetX: 9,
          offsetY: -2,
        },
      },
    },
  ];
  return vectors.map(({ name, shape, start, end, style, opacity }) => {
    const request = { shape, start, end };
    if (style) request.style = style;
    if (opacity !== undefined) request.opacity = opacity;
    const element = {
      id: 'fixture-created-shape',
      kind: 'shape',
      shape,
      x: start.x,
      y: start.y,
      endX: end.x,
      endY: end.y,
      controls: [],
      style: style ?? defaults,
      locked: false,
      visible: true,
      opacity: opacity ?? 100,
      blendMode: 'source-over',
    };
    let expected = { ...structuredClone(input), elements: [...structuredClone(input.elements), element] };
    const bounds = elementBounds(element);
    if (isFullyOutsideCanvas(bounds, expected)) {
      expected = expandDocumentToFitBounds(expected, bounds, 0);
    }
    return { name, input, request, expected };
  });
}

function openShapeCreationCases() {
  const input = {
    width: 40,
    height: 30,
    background: '#ffffff',
    futureDocument: { preserve: 'open-creation' },
    elements: [{
      id: 'existing-hidden-locked',
      kind: 'image',
      source: 'imported',
      src: 'draft-asset:existing',
      originalSrc: null,
      name: 'existing.png',
      sourceArtifactId: null,
      x: 1.25,
      y: -2.5,
      width: 3,
      height: 2,
      naturalWidth: 3,
      naturalHeight: 2,
      locked: true,
      visible: false,
      opacity: 63,
      blendMode: 'screen',
      futureImage: ['keep'],
    }],
  };
  const defaults = {
    color: '#ff3b5c',
    fill: '#ff3b5c',
    strokeWidth: 8,
    strokeEnabled: false,
    dropShadow: false,
  };
  const vectors = [
    {
      name: 'zero length fractional line remains a valid round dot',
      shape: 'line',
      start: { x: 5.25, y: 4.75 },
      end: { x: 5.25, y: 4.75 },
    },
    {
      name: 'horizontal line keeps signed partially clipped endpoints',
      shape: 'line',
      start: { x: 13.75, y: 8.5 },
      end: { x: -3.25, y: 8.5 },
    },
    {
      name: 'vertical line fully outside expands without losing fractions',
      shape: 'line',
      start: { x: 46.25, y: 34.5 },
      end: { x: 46.25, y: 43.75 },
    },
    {
      name: 'reverse fractional arrow stays clipped when partially overlapping',
      shape: 'arrow',
      start: { x: 20.75, y: 15.25 },
      end: { x: -3.125, y: 4.875 },
    },
    {
      name: 'arrow immediately below the intrinsic paint boundary is rejected',
      shape: 'arrow',
      start: { x: 7.25, y: 9.5 },
      end: { x: 8.749, y: 9.5 },
    },
    {
      name: 'arrow exactly at the intrinsic paint boundary is retained',
      shape: 'arrow',
      start: { x: 7.25, y: 9.5 },
      end: { x: 8.75, y: 9.5 },
    },
    {
      name: 'default shadow-only line overlap remains clipped',
      shape: 'line',
      start: { x: -24.25, y: 12.5 },
      end: { x: -21.25, y: 18.75 },
      style: { ...defaults, dropShadow: true },
    },
    {
      name: 'custom shadow fully outside arrow expands and translates siblings',
      shape: 'arrow',
      start: { x: -55.5, y: -38.25 },
      end: { x: -39.25, y: -29.75 },
      style: {
        color: '#2277dd',
        fill: '#00ff00',
        strokeWidth: 6,
        strokeEnabled: true,
        dropShadow: true,
        dropShadowStyle: {
          color: '#112233',
          opacity: 80,
          blur: 4,
          offsetX: 9,
          offsetY: -2,
        },
        futureStyle: { preserve: 'open' },
      },
      opacity: 62.5,
    },
  ];
  return vectors.map(({ name, shape, start, end, style, opacity }) => {
    const request = { shape, start, end };
    if (style) request.style = style;
    if (opacity !== undefined) request.opacity = opacity;
    const element = {
      id: 'fixture-created-open-shape',
      kind: 'shape',
      shape,
      x: start.x,
      y: start.y,
      endX: end.x,
      endY: end.y,
      controls: [],
      style: { ...(style ?? defaults), fill: null },
      locked: false,
      visible: true,
      opacity: opacity ?? 100,
      blendMode: 'source-over',
    };
    const pathLength = arrowPathLength(element);
    if (shape === 'arrow' && pathLength < ARROW_MIN_DRAW_LENGTH) {
      return { name, input, request, pathLength, expected: null };
    }
    let expected = { ...structuredClone(input), elements: [...structuredClone(input.elements), element] };
    const bounds = elementBounds(element);
    if (isFullyOutsideCanvas(bounds, expected)) {
      expected = expandDocumentToFitBounds(expected, bounds, 0);
    }
    return { name, input, request, pathLength, expected };
  });
}

function freehandCreationCases() {
  const input = {
    width: 40,
    height: 30,
    background: '#ffffff',
    futureDocument: { preserve: 'freehand-creation' },
    elements: [{
      id: 'existing-hidden-locked',
      kind: 'image',
      source: 'imported',
      src: 'draft-asset:existing',
      originalSrc: null,
      name: 'existing.png',
      sourceArtifactId: null,
      x: 1.25,
      y: -2.5,
      width: 3,
      height: 2,
      naturalWidth: 3,
      naturalHeight: 2,
      locked: true,
      visible: false,
      opacity: 63,
      blendMode: 'screen',
      futureImage: ['keep'],
    }],
  };
  const defaults = {
    color: '#ff3b5c',
    fill: '#ff3b5c',
    strokeWidth: 8,
    strokeEnabled: false,
    dropShadow: false,
  };
  const vectors = [
    {
      name: 'single fractional sample remains a round dot',
      points: [{ x: 5.25, y: 4.75 }],
    },
    {
      name: 'repeated samples are preserved in authored order',
      points: [{ x: 11.25, y: 8.5 }, { x: 11.25, y: 8.5 }, { x: 17.75, y: 13.125 }],
    },
    {
      name: 'curved sample hull rather than smoothed centerline drives bounds',
      points: [{ x: 8.25, y: 19.5 }, { x: 19.75, y: -4.25 }, { x: 31.125, y: 18.75 }],
      style: { ...defaults, strokeWidth: 2.5 },
    },
    {
      name: 'negative fractional partial overlap remains clipped',
      points: [{ x: -7.75, y: 3.25 }, { x: 1.125, y: 7.5 }, { x: -3.5, y: 14.875 }],
    },
    {
      name: 'default shadow-only overlap remains clipped',
      points: [{ x: -27.5, y: 9.25 }],
      style: { ...defaults, dropShadow: true },
    },
    {
      name: 'custom shadow fully outside path expands and translates every sample and sibling',
      points: [{ x: -58.5, y: -39.25 }, { x: -48.75, y: -32.5 }, { x: -42.125, y: -36.75 }],
      style: {
        color: '#2277dd',
        fill: '#00ff00',
        strokeWidth: 3.5,
        strokeEnabled: true,
        dropShadow: true,
        dropShadowStyle: {
          color: '#112233',
          opacity: 80,
          blur: 4,
          offsetX: 9,
          offsetY: -2,
        },
        futureStyle: { preserve: 'freehand' },
      },
      opacity: 62.5,
    },
  ];
  return vectors.map(({ name, points, style, opacity }) => {
    const request = { points };
    if (style) request.style = style;
    if (opacity !== undefined) request.opacity = opacity;
    const element = {
      id: 'fixture-created-freehand-path',
      kind: 'path',
      x: points[0].x,
      y: points[0].y,
      points,
      style: { ...(style ?? defaults), fill: null },
      locked: false,
      visible: true,
      opacity: opacity ?? 100,
      blendMode: 'source-over',
    };
    let expected = { ...structuredClone(input), elements: [...structuredClone(input.elements), element] };
    const bounds = elementBounds(element);
    if (isFullyOutsideCanvas(bounds, expected)) {
      expected = expandDocumentToFitBounds(expected, bounds, 0);
    }
    return { name, input, request, expected };
  });
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
    `  "cropDrags": ${array(cases.cropDrags, '    ')},`,
    `  "translations": ${array(cases.translations, '    ')},`,
    `  "cropRects": ${array(cases.cropRects, '    ')},`,
    `  "canvasSizes": ${array(cases.canvasSizes, '    ')},`,
    `  "shapeCreations": ${array(cases.shapeCreations, '    ')},`,
    `  "openShapeCreations": ${array(cases.openShapeCreations, '    ')},`,
    `  "freehandCreations": ${array(cases.freehandCreations, '    ')},`,
    `  "hitTests": ${array(cases.hitTests, '    ')},`,
    `  "orientations": ${array(cases.orientations, '    ')},`,
    `  "layers": ${JSON.stringify(cases.layers)},`,
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
  test('selection vectors distinguish local edges, layer order and transparent content', () => {
    const cases = hitTestCases();
    for (const entry of cases.filter(entry => entry.bounds)) {
      assert.equal(entry.queries.filter(query => query.expected !== null).length, 8);
      assert.equal(entry.queries.filter(query => query.expected === null).length, 8);
    }
    const expected = name => cases.find(entry => entry.name === name).queries.map(query => query.expected);
    assert.deepEqual(expected('frontmost'), ['front']);
    assert.deepEqual(expected('hidden-locked'), ['image']);
    assert.deepEqual(expected('transparent-still-selectable'), ['transparent']);
    assert.deepEqual(expected('miss-front-hit-back'), ['image']);
    assert.deepEqual(expected('empty'), [null]);
    assert.deepEqual(expected('inclusive-image-edge'), ['image', 'image']);
    assert.deepEqual(expected('fractional-unrotated-edge'), ['image']);
  });
  test('vectors cover geometry, every D4 state, unknown data, and history boundaries', () => {
    const vectors = shippingCases();
    assert.equal(vectors.orientations.length, 32);
    assert.equal(vectors.layers.reorders.length, 50);
    assert.equal(vectors.layers.duplicates.length, 4);
    assert.ok(vectors.document.elements.some(element => element.visible === false));
    assert.ok(vectors.document.elements.some(element => element.locked === true));
    assert.ok(vectors.crops.some(entry => entry.start.x < 0));
    assert.ok(vectors.cropDrags.some(entry => entry.updates.some(step => step.shiftKey)));
    assert.equal(vectors.shapeCreations.length, 6);
    assert.ok(vectors.shapeCreations.some(entry => entry.expected.width > entry.input.width));
    assert.ok(vectors.shapeCreations.some(entry => entry.expected.elements[0].x > entry.input.elements[0].x));
    assert.equal(vectors.openShapeCreations.length, 8);
    assert.ok(vectors.openShapeCreations.some(entry => entry.request.start.x === entry.request.end.x));
    assert.ok(vectors.openShapeCreations.some(entry => entry.request.start.y === entry.request.end.y));
    assert.ok(vectors.openShapeCreations.some(entry => entry.expected === null));
    assert.ok(vectors.openShapeCreations.some(entry => entry.expected?.elements[0].x > entry.input.elements[0].x));
    assert.equal(vectors.freehandCreations.length, 6);
    assert.ok(vectors.freehandCreations.some(entry => entry.request.points.length === 1));
    assert.ok(vectors.freehandCreations.some(entry => entry.request.points[0].x < 0));
    assert.ok(vectors.freehandCreations.some(entry => entry.expected.elements[0].x > entry.input.elements[0].x));
    assert.ok(vectors.history.expected.some(entry => entry.undo === 100));
    assert.ok(vectors.history.expected.some(entry => entry.redo === 100));
    assert.equal(vectors.history.expected[0].changed, false);
  });
}
