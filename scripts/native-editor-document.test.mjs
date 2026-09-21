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
  collectAlignmentSnapLines,
  duplicateScreenshotElement,
  elementBounds,
  elementLocalBounds,
  elementRotation,
  elementRotationHandleAnchorPoint,
  elementRotationHandleFitsCanvas,
  elementRotationHandlePoint,
  elementWorldPoint,
  expandDocumentToFitBounds,
  hitTestElement,
  hitTestResizeHandle,
  isFullyOutsideCanvas,
  elementLocalPoint,
  oppositeResizeHandle,
  resizeBoundsFromHandle,
  resizeElement,
  resizeHandlePoint,
  reorderScreenshotLayers,
  resizeDocumentCanvas,
  snapResizedBounds,
  snapTranslatedBounds,
  snapShapeRotation,
  transformImageElement,
  translateElement,
  trimDocumentToContent,
  withElementRotation,
  preserveElementWorldPoint,
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
  const image = { ...document.elements[0], visible: true, locked: true, opacity: 0 };
  const trims = [
    ['empty', []],
    ['hidden-only', [document.elements[0]]],
    ['locked-transparent-fractional-overhang', [image]],
    ['subpixel-minimum-extent', [{ ...image, x: 2.2, y: -3.7, width: 0.2, height: 0.3 }]],
    ['rotated-image-and-hidden-sibling', [{ ...image, rotation: 0.67 }, document.elements[0]]],
    ['rotated-shape-shadow', [{ ...document.elements[2], rotation: -0.39,
      style: { ...style, dropShadow: true } }]],
    ['rotated-path-shadow', [{ ...document.elements[3], rotation: 0.41 }]],
    ['mixed-visible-with-hidden-sibling', [image, document.elements[2], document.elements[3],
      { ...image, id: 'hidden', visible: false, x: -900, y: 1000 }]],
    ['already-tight', [{ ...image, x: 0, y: 0, width: 713, height: 257 }]],
  ].map(([name, elements]) => {
    const input = { ...document, elements };
    return { name, input, expected: trimDocumentToContent(input) };
  });
  return { crops, cropDrags: cropDragCases(), translations, cropRects, canvasSizes, trims };
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

function resizeOutline(element) {
  const bounds = elementLocalBounds(element);
  return [
    { x: bounds.x, y: bounds.y },
    { x: bounds.x + bounds.width, y: bounds.y },
    { x: bounds.x + bounds.width, y: bounds.y + bounds.height },
    { x: bounds.x, y: bounds.y + bounds.height },
  ].map(point => elementWorldPoint(element, point));
}

function resizeOracle(input, id, handle, displayScale, current, lockAspect) {
  const element = input.elements.find(candidate => candidate.id === id);
  assert.ok(element);
  const initialBounds = elementLocalBounds(element);
  const minSize = 8 / Math.max(0.01, displayScale);
  const threshold = 10 / Math.max(0.01, displayScale);
  const pointer = elementLocalPoint(element, current);
  const free = resizeBoundsFromHandle(
    initialBounds, handle, pointer, minSize, lockAspect,
  );
  const lines = collectAlignmentSnapLines(input, id);
  const snapped = elementRotation(element) !== 0
    ? { bounds: free, guides: [] }
    : snapResizedBounds(initialBounds, handle, free, lines, threshold, minSize);
  const nextBounds = lockAspect
    ? resizeBoundsFromHandle(
      initialBounds, handle, resizeHandlePoint(snapped.bounds, handle), minSize, true,
    )
    : snapped.bounds;
  let resized = resizeElement(element, initialBounds, nextBounds);
  if (elementRotation(resized) !== 0) {
    resized = preserveElementWorldPoint(
      element, resized, resizeHandlePoint(initialBounds, oppositeResizeHandle(handle)),
    );
  }
  let committed = {
    ...input,
    elements: input.elements.map(candidate => candidate.id === id ? resized : candidate),
  };
  const painted = elementBounds(resized);
  if (isFullyOutsideCanvas(painted, committed)) {
    committed = expandDocumentToFitBounds(committed, painted, 0);
  }
  return {
    element: resized,
    outline: resizeOutline(resized),
    guides: snapped.guides,
    committed,
  };
}

function moveOracle(input, id, displayScale, delta) {
  const element = input.elements.find(candidate => candidate.id === id);
  assert.ok(element);
  const free = translateElement(element, delta.x, delta.y);
  const freeBounds = elementBounds(free);
  const lines = collectAlignmentSnapLines(input, id);
  const snapped = snapTranslatedBounds(
    freeBounds,
    lines,
    10 / Math.max(0.01, displayScale),
  );
  const moved = translateElement(
    free,
    snapped.bounds.x - freeBounds.x,
    snapped.bounds.y - freeBounds.y,
  );
  let committed = {
    ...input,
    elements: input.elements.map(candidate => candidate.id === id ? moved : candidate),
  };
  const painted = elementBounds(moved);
  if (isFullyOutsideCanvas(painted, committed)) {
    committed = expandDocumentToFitBounds(committed, painted, 0);
  }
  return {
    element: moved,
    outline: resizeOutline(moved),
    guides: snapped.guides,
    committed,
  };
}

function moveCases() {
  const image = {
    ...document.elements[0], id: 'move-image', visible: true, x: 41.25, y: 37.5,
    width: 83.5, height: 46.25, orientation: 'transverse', futureMove: { keep: true },
  };
  const shape = {
    ...document.elements[2], id: 'move-shape', shape: 'rectangle', x: 52.25, y: 61.5,
    endX: 137.75, endY: 105.25, controls: [], rotation: 0.47,
    style: { ...style, strokeWidth: 11.5, dropShadow: true }, futureMove: ['shape'],
  };
  const path = {
    ...document.elements[3], id: 'move-path', x: 34.5, y: 45.25,
    points: [{ x: 34.5, y: 45.25 }, { x: 91.75, y: 128.5 }, { x: 157.25, y: 69.75 }],
    rotation: -0.39, style: { ...style, strokeWidth: 7.5, dropShadow: true },
    futureMove: { path: 4 },
  };
  const arrow = {
    ...document.elements[2], id: 'move-arrow', x: 43.25, y: 94.5,
    endX: 176.75, endY: 37.25, controls: [{ x: 88.5, y: 22.75 }], rotation: 0.31,
    style: { ...style, strokeWidth: 13.25, dropShadow: true }, futureMove: { arrow: true },
  };
  const sibling = (id, x, y, width = 35, height = 27, extra = {}) => ({
    ...image, id, x, y, width, height, orientation: undefined, futureMove: { sibling: id }, ...extra,
  });
  const base = { width: 300, height: 220, background: null, futureMoveDocument: 4, elements: [image] };
  const drags = [];
  const add = (name, input, element, delta, displayScale = 1) => {
    const oracle = moveOracle(input, element.id, displayScale, delta);
    drags.push({ name, input, id: element.id, displayScale, delta,
      expected: { element: oracle.element, outline: oracle.outline, guides: oracle.guides },
      committed: oracle.committed });
  };

  add('asymmetric image free xy', base, image, { x: 23.125, y: -17.375 });
  add('left and top canvas edges', base, image, { x: -39.5, y: -35.25 });
  add('right and bottom canvas edges', base, image, { x: 167.2, y: 138.1 });
  add('exact threshold does not snap', base, image, { x: -31.25, y: -27.5 });
  add('just inside threshold snaps', base, image, { x: -31.251, y: -27.501 });
  add('threshold scales with display', base, image, { x: -36.251, y: -32.501 }, 2);

  const vertical = sibling('vertical', 160, 18, 40, 60);
  const horizontal = sibling('horizontal', 12, 150, 60, 30);
  add('sibling left and top axes', { ...base, elements: [image, vertical, horizontal] }, image,
    { x: 110.1, y: 102.2 });
  add('sibling right and bottom axes', { ...base, elements: [image, vertical, horizontal] }, image,
    { x: 35.2, y: 66.1 });
  add('cross edge right to left and bottom to top', { ...base, elements: [image, vertical, horizontal] }, image,
    { x: 35.1, y: 66.2 });

  const sameSize = sibling('same-size', 190, 130, image.width, image.height);
  add('same width and height lights both edges', { ...base, elements: [image, sameSize] }, image,
    { x: 148.4, y: 92.4 });
  const tieEarlier = sibling('tie-earlier', 100, 20, 20, 20);
  const tieLater = sibling('tie-later', 102, 90, 20, 20);
  add('line tie chooses later line', { ...base, elements: [image, tieEarlier, tieLater] }, image,
    { x: 59.75, y: 17.375 });
  const edgeTie = sibling('edge-tie', 100, 100, image.width + 2, image.height + 2);
  add('edge tie keeps first moving edge', { ...base, elements: [image, edgeTie] }, image,
    { x: 59.75, y: 62.5 });

  const locked = sibling('locked-sibling', 170, 120, 35, 27, { locked: true });
  const hidden = sibling('hidden-sibling', 90, 80, 35, 27, { visible: false });
  const transparent = sibling('transparent-sibling', 230, 40, 35, 27, { opacity: 0 });
  add('visible locked sibling participates', { ...base, elements: [image, locked] }, image,
    { x: 119.1, y: 82.2 });
  add('hidden sibling is ignored', { ...base, elements: [image, hidden] }, image,
    { x: 39.1, y: 42.2 });
  add('zero opacity visible sibling participates', { ...base, elements: [image, transparent] }, image,
    { x: 179.1, y: 2.2 });

  add('rotated painted shape bounds snap', { ...base, elements: [shape, vertical] }, shape,
    { x: 20.3, y: -29.4 });
  add('rotated painted path bounds snap', { ...base, elements: [path, horizontal] }, path,
    { x: 17.2, y: 15.3 });
  add('rotated painted arrow bounds snap', { ...base, elements: [arrow, vertical] }, arrow,
    { x: -11.4, y: 22.6 });

  const expansionSibling = sibling('expansion-sibling', 2, 3, 5, 4, { locked: true });
  add('partial positive overflow stays clipped',
    { ...base, width: 80, height: 60, elements: [expansionSibling, image] }, image,
    { x: 20, y: 5 });
  add('fully lost positive expands without shifting sibling',
    { ...base, width: 80, height: 60, elements: [expansionSibling, image] }, image,
    { x: 100, y: 75 });
  add('fully lost negative expands and shifts sibling',
    { ...base, width: 80, height: 60, elements: [expansionSibling, image] }, image,
    { x: -150, y: -110 });
  add('partial negative overflow stays clipped',
    { ...base, width: 80, height: 60, elements: [expansionSibling, image] }, image,
    { x: -70, y: -50 });

  add('image D4 and metadata preserved', base, image, { x: 7.125, y: 13.75 }, 1.25);
  add('shape metadata preserved', { ...base, elements: [shape] }, shape, { x: 9.5, y: 11.25 });
  add('path metadata preserved', { ...base, elements: [path] }, path, { x: -9.5, y: 8.25 });
  add('arrow metadata preserved', { ...base, elements: [arrow] }, arrow, { x: 12.5, y: -8.25 });
  return drags;
}

function resizeCases() {
  const image = {
    ...document.elements[0], id: 'resize-image', visible: true, x: 61.25, y: 47.5,
    width: 83.5, height: 46.25, orientation: 'rotate-90', futureResize: { keep: true },
  };
  const arrow = {
    ...document.elements[2], id: 'resize-arrow', x: 52.25, y: 71.5,
    endX: 174.75, endY: 112.25, controls: [{ x: 91.5, y: 31.75 }, { x: 139.25, y: 146.5 }],
    style: { ...style, strokeWidth: 13.4 }, futureResize: ['arrow'],
  };
  const path = {
    ...document.elements[3], id: 'resize-path', x: 44.5, y: 55.25,
    points: [{ x: 44.5, y: 55.25 }, { x: 91.75, y: 128.5 }, { x: 157.25, y: 69.75 }],
    style: { ...style, strokeWidth: 6.5 }, futureResize: { path: 1 },
  };
  const baseDocument = {
    width: 300, height: 220, background: null, futureResizeDocument: true,
    elements: [image],
  };
  const handles = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];
  const hitBounds = elementLocalBounds(image);
  const hitTests = [{
    name: 'all eight asymmetric image handles plus miss',
    element: image,
    radius: 5.25,
    queries: [
      ...handles.map(handle => ({
        point: elementWorldPoint(image, resizeHandlePoint(hitBounds, handle)),
        expected: hitTestResizeHandle(hitBounds, resizeHandlePoint(hitBounds, handle), 5.25),
      })),
      { point: elementWorldPoint(image, { x: hitBounds.x + 20, y: hitBounds.y + 17 }), expected: null },
    ],
  }];
  for (const rotation of [0, -0.67]) {
    const element = { ...image, rotation };
    const localPoints = [-0.125, 0.125].flatMap(offset => [
      { x: hitBounds.x - 5.25 + offset, y: hitBounds.y },
      { x: hitBounds.x + 21, y: hitBounds.y - 5.25 + offset },
      { x: hitBounds.x + hitBounds.width + 5.25 + offset, y: hitBounds.y + 18 },
      { x: hitBounds.x + 21, y: hitBounds.y + hitBounds.height + 5.25 + offset },
    ]);
    hitTests.push({ name: `corner and border boundaries at rotation ${rotation}`, element, radius: 5.25,
      queries: localPoints.map(local => {
        const point = elementWorldPoint(element, local);
        return { point, expected: hitTestResizeHandle(hitBounds, elementLocalPoint(element, point), 5.25) };
      }) });
  }
  const drags = [];
  const add = (name, input, element, handle, current, lockAspect = false, displayScale = 1) => {
    const oracle = resizeOracle(input, element.id, handle, displayScale, current, lockAspect);
    drags.push({ name, input, id: element.id, handle, displayScale, current, lockAspect,
      expected: { element: oracle.element, outline: oracle.outline, guides: oracle.guides },
      committed: oracle.committed });
  };

  // Every handle gets asymmetric movement. Explicit Shift on an edge must still
  // remain single-axis; only corners lock aspect ratio.
  handles.forEach((handle, index) => {
    const point = resizeHandlePoint(hitBounds, handle);
    add(`asymmetric ${handle}`, baseDocument, image, handle,
      { x: point.x + 17.25 - index * 2.1, y: point.y - 13.75 + index * 1.3 },
      ['nw', 'ne', 'se', 'sw'].includes(handle));
  });
  add('edge shift remains single axis', baseDocument, image, 'e', { x: 193.25, y: 3.5 }, true);
  add('crosses fixed anchor', baseDocument, image, 'nw', { x: 190.25, y: 137.75 });
  add('minimum size at zoom', baseDocument, image, 'se', { x: 63, y: 49 }, false, 2);

  const rotated = { ...image, id: 'rotated', rotation: 0.63, orientation: 'transverse' };
  add('rotated corner preserves opposite world anchor', { ...baseDocument, elements: [rotated] },
    rotated, 'ne', { x: 207.5, y: 29.25 }, true, 1.25);
  add('rotated edge preserves opposite world anchor', { ...baseDocument, elements: [rotated] },
    rotated, 's', { x: 117.5, y: 174.25 }, false, 0.8);

  const orientations = [undefined, 'rotate-90', 'rotate-180', 'rotate-270',
    'flip-horizontal', 'flip-vertical', 'transpose', 'transverse'];
  orientations.forEach((orientation, index) => {
    const d4 = { ...image, id: `d4-${index}` };
    if (orientation === undefined) delete d4.orientation;
    else d4.orientation = orientation;
    const input = { ...baseDocument, elements: [d4] };
    add(`D4 ${orientation ?? 'identity'}`, input, d4, handles[index],
      { x: 35.25 + index * 19.5, y: 28.75 + index * 14.25 }, index % 2 === 0);
  });

  add('curved arrow scales controls and rounds clamped stroke', { ...baseDocument, elements: [arrow] },
    arrow, 'se', { x: 81.25, y: 88.75 }, false);
  add('one axis arrow enlargement retains fractional stroke', { ...baseDocument, elements: [arrow] },
    arrow, 'e', { x: 250.25, y: 80.5 });
  for (const kind of ['rectangle', 'ellipse']) {
    const closed = { ...arrow, id: kind, shape: kind, controls: [], rotation: -0.47 };
    add(`rotated ${kind} retains stroke padding`, { ...baseDocument, elements: [closed] },
      closed, 'sw', { x: 63.25, y: 176.5 }, true);
  }
  add('path scales points but retains stroke', { ...baseDocument, elements: [path] },
    path, 'w', { x: 11.25, y: 93.5 }, false);
  const dot = { ...path, id: 'dot-path', x: 80.25, y: 60.5, points: [{ x: 80.25, y: 60.5 }] };
  add('single point path', { ...baseDocument, elements: [dot] }, dot, 'se', { x: 128.5, y: 109.75 });
  const empty = { ...path, id: 'empty-path', x: 80.25, y: 60.5, points: [] };
  add('empty path', { ...baseDocument, elements: [empty] }, empty, 'nw', { x: 53.25, y: 44.75 });

  const target = { ...image, id: 'target', x: 40, y: 50, width: 40, height: 30 };
  const visible = { ...image, id: 'visible', x: 120, y: 100, width: 20, height: 20 };
  const locked = { ...image, id: 'locked', x: 180, y: 140, width: 20, height: 15, locked: true };
  const hidden = { ...image, id: 'hidden', x: 90, y: 80, width: 10, height: 10, visible: false };
  const snapDocument = { ...baseDocument, width: 210, height: 180,
    elements: [target, visible, locked, hidden] };
  add('snap threshold inclusive and visible layer', snapDocument, target, 'se', { x: 110, y: 90 });
  add('snap closest tie chooses later line', snapDocument, target, 'e', { x: 130, y: 65 });
  add('canvas origin snapping', snapDocument,
    target, 'nw', { x: 9.75, y: 9.75 }, false);
  add('just outside snap threshold', snapDocument, target, 'se', { x: 109.875, y: 89.875 });
  add('locked visible contributes', snapDocument, target, 'se', { x: 171, y: 131 });
  add('hidden edges are excluded', snapDocument, target, 'se', { x: 99, y: 89 });

  const outside = { ...image, id: 'outside', x: 100, y: 75, width: 30, height: 20 };
  const partial = { ...outside, x: 20, y: 20 };
  const sibling = { ...image, id: 'sibling', x: 2, y: 3, width: 5, height: 4, locked: true };
  add('fully outside expands and translates siblings',
    { ...baseDocument, width: 80, height: 60, elements: [sibling, outside] },
    outside, 'se', { x: 145, y: 112 });
  add('partial overlap remains clipped',
    { ...baseDocument, width: 80, height: 60, elements: [sibling, partial] },
    partial, 'nw', { x: -30, y: -20 });
  const negative = { ...outside, x: -80, y: -60, width: 20, height: 10 };
  add('negative outside resize translates siblings',
    { ...baseDocument, width: 80, height: 60, elements: [sibling, negative] },
    negative, 'nw', { x: -115, y: -90 });
  return { hitTests, drags };
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
    moves: moveCases(),
    resize: resizeCases(),
    rotations: rotationCases(),
    orientations: orientationCases(),
    layers,
    history: historyCases(),
  };
}

function rotationCases() {
  const image = { ...document.elements[0], x: 0, y: 0, width: 713, height: 257, visible: true };
  const elements = [image, ...hitTestCases().filter(value => value.outline).map(value => value.input.elements[0])];
  const outlines = element => {
    const bounds = elementLocalBounds(element);
    return [
      { x: bounds.x, y: bounds.y }, { x: bounds.x + bounds.width, y: bounds.y },
      { x: bounds.x + bounds.width, y: bounds.y + bounds.height }, { x: bounds.x, y: bounds.y + bounds.height },
    ].map(point => elementWorldPoint(element, point));
  };
  const angles = [-Math.PI * 7, -Math.PI, Math.PI, Math.PI * 9, -1e-11, 1e-11,
    -Math.PI / 24, Math.PI / 24, -0.131, -0.1308, 0.1308, 0.131, 0.73]
    .flatMap(radians => [false, true].map(snap => ({ radians, snap, expected: snapShapeRotation(radians, snap) })));
  const handles = elements.flatMap(element => [0.5, 1, 2].flatMap(scale => [
    { width: 713, height: 257 }, { width: 1, height: 1 },
  ].map(canvas => ({
    outline: outlines(element), radians: elementRotation(element), scale, canvas,
    expected: elementRotationHandleFitsCanvas(element, scale, canvas) ? {
      anchor: elementRotationHandleAnchorPoint(element, scale, canvas),
      handle: elementRotationHandlePoint(element, scale, canvas), hit_radius: 12.5 / scale,
    } : null,
  }))));
  const gestures = elements.flatMap(element => [false, true].map(snap => {
    const local = elementLocalBounds(element);
    const origin = { x: local.x + local.width / 2, y: local.y + local.height / 2 };
    const start = { x: origin.x - 31, y: origin.y - 77 };
    const current = { x: origin.x + 53, y: origin.y - 19 };
    const radians = snapShapeRotation(elementRotation(element)
      + Math.atan2(current.y - origin.y, current.x - origin.x)
      - Math.atan2(start.y - origin.y, start.x - origin.x), snap);
    return { outline: outlines(element), initial: elementRotation(element), start, current, snap,
      expected: { radians, outline: outlines(withElementRotation(element, radians)) } };
  }));
  const edits = elements.flatMap(element => [0, Math.PI / 2, -0.71].map(radians => {
    const input = { ...document, elements: [element] };
    let expected = { ...input, elements: [withElementRotation(element, radians)] };
    const bounds = elementBounds(expected.elements[0]);
    if (isFullyOutsideCanvas(bounds, expected)) expected = expandDocumentToFitBounds(expected, bounds, 0);
    return { input, id: element.id, radians, expected };
  }));
  // Rotation can move a partially overlapping layer fully outside. Path stroke
  // padding also extends above zero, so expansion must translate every sibling.
  const overflowing = [
    { ...image, x: 80, y: 45, width: 100, height: 10 },
    { ...document.elements[2], shape: 'rectangle', x: 80, y: 45, endX: 180, endY: 55, controls: [] },
    { ...document.elements[3], x: 80, y: 45, points: [{ x: 80, y: 45 }, { x: 180, y: 55 }], style },
  ];
  edits.push(...overflowing.map(element => {
    const sibling = { ...image, id: 'sibling', x: 0, y: 0, width: 4, height: 3, locked: true };
    const input = { ...document, width: 100, height: 100, elements: [sibling, element] };
    const radians = Math.PI / 2;
    const rotated = { ...input, elements: [sibling, withElementRotation(element, radians)] };
    const bounds = elementBounds(rotated.elements[1]);
    assert.equal(isFullyOutsideCanvas(elementBounds(element), input), false);
    assert.equal(isFullyOutsideCanvas(bounds, rotated), true);
    const expected = expandDocumentToFitBounds(rotated, bounds, 0);
    assert.ok(expected.width > input.width);
    if (element.kind === 'path') assert.ok(expected.elements[0].y > sibling.y);
    return { input, id: element.id, radians, expected };
  }));
  return { angles, handles, gestures, edits };
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
    `  "trims": ${array(cases.trims, '    ')},`,
    `  "shapeCreations": ${array(cases.shapeCreations, '    ')},`,
    `  "openShapeCreations": ${array(cases.openShapeCreations, '    ')},`,
    `  "freehandCreations": ${array(cases.freehandCreations, '    ')},`,
    `  "hitTests": ${array(cases.hitTests, '    ')},`,
    `  "moves": ${array(cases.moves, '    ')},`,
    '  "resize": {',
    `    "hitTests": ${array(cases.resize.hitTests, '      ')},`,
    `    "drags": ${array(cases.resize.drags, '      ')}`,
    '  },',
    '  "rotations": {',
    `    "angles": ${array(cases.rotations.angles, '      ')},`,
    `    "handles": ${array(cases.rotations.handles, '      ')},`,
    `    "gestures": ${array(cases.rotations.gestures, '      ')},`,
    `    "edits": ${array(cases.rotations.edits, '      ')}`,
    '  },',
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
  test('trim rounds outward, includes transparent locked geometry, and ignores hidden-only documents', () => {
    const cases = geometryCases().trims;
    const fractional = cases.find(entry => entry.name === 'locked-transparent-fractional-overhang').expected;
    assert.deepEqual([fractional.width, fractional.height, fractional.elements[0].x,
      fractional.elements[0].y], [93, 44, 0.75, 0.75]);
    const subpixel = cases.find(entry => entry.name === 'subpixel-minimum-extent').expected;
    assert.deepEqual([subpixel.width, subpixel.height], [2, 2]);
    for (const name of ['empty', 'hidden-only', 'already-tight']) {
      const entry = cases.find(entry => entry.name === name);
      assert.equal(entry.expected, entry.input);
    }
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
    assert.equal(vectors.resize.hitTests[0].queries.length, 9);
    assert.equal(vectors.resize.drags.length, 37);
    assert.equal(vectors.moves.length, 26);
    assert.deepEqual(
      new Set(vectors.resize.drags.slice(0, 8).map(entry => entry.handle)),
      new Set(['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w']),
    );
    assert.ok(vectors.history.expected.some(entry => entry.undo === 100));
    assert.ok(vectors.history.expected.some(entry => entry.redo === 100));
    assert.equal(vectors.history.expected[0].changed, false);
  });
}
