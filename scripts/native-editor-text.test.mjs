import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import test from 'node:test';
import {
  fitAutoWidthTextElement, fitEditingAutoWidthTextElement,
  textBackgroundPad, textBackgroundRadius, textHasBackgroundPlate,
  TEXT_LINE_HEIGHT_RATIO, wrapTextLines,
} from '../apps/desktop/ui/src/lib/screenshotEditor.ts';

const fixture = new URL('../crates/captures-app/tests/editor-text-golden.json', import.meta.url);
// Deliberately asymmetric, non-additive measurements. The callback is a test
// oracle, not a production substitute for shaping. Rust consumes this table.
const widths = { A: 7, B: 11, C: 5, i: 2, f: 4, ' ': 3, '😀': 13, '\u0301': 0 };
const measure = line => [...line.replaceAll('fi', '§')]
  .reduce((sum, c) => sum + (c === '§' ? 4.5 : widths[c] ?? 9), 0);

function cases() {
  const variants = [
    { text: '  A  B  \n\nC\n', width: 30 },
    { text: 'A B', width: 21, align: 'center' },
    { text: 'A B', width: 20.999, align: 'right' },
    { text: 'AB😀Cfi', width: 18, align: 'right' },
    { text: 'A\u0301B', width: 8, background: null },
    { text: '\ufeff A\ufeffB\ufeff', width: 60, background: '' },
    { text: '\u0085A\u0085', width: 70, roundedBackground: false },
    { text: '\tA\t\tB\r\nC', width: 80 },
    { text: '\u00a0A\u2009B\u3000', width: 100 },
    { text: '', width: 1, fontSize: 13.25, align: 'center' },
    { text: 'i', width: 1, fontSize: 17, align: 'right' },
    { text: 'fi\nAB', autoWidth: true, width: 81, align: 'left' },
    { text: 'fi\nAB', autoWidth: true, width: 81, align: 'center' },
    { text: 'fi\nAB', autoWidth: true, width: 81, align: 'right' },
    { text: 'A', autoWidth: true, width: 14.49, fontSize: 20, align: 'right' },
    { text: 'A', autoWidth: true, width: 14.5, fontSize: 20, align: 'right' },
    { text: '\ufeff \n', autoWidth: true, width: 40, align: 'center' },
    { text: '\u0085', autoWidth: true, width: 40, align: 'right' },
  ];
  return variants.map((variant, index) => {
    const element = {
      id: `text-${index}`, kind: 'text', x: -13.25, y: 17.75, rotation: 0.3,
      visible: true, locked: true, opacity: 61, blendMode: 'multiply',
      text: '', fontSize: 23.5, width: 50, fontFamily: 'explicit-test-face',
      bold: true, italic: true, align: 'left', color: '#2174c5',
      background: '#e2e4e6', roundedBackground: true, outlined: false,
      dropShadow: true, futureMetadata: { preserve: [1, 'two'] }, ...variant,
    };
    const measurements = {};
    const recordedMeasure = line => (measurements[line] = measure(line));
    // Mirror drawText's paint coordinates; wrap/auto-fit/plate rules themselves
    // run the actual shipping helpers, not a second copy of their algorithms.
    const width = Math.max(element.fontSize * 0.5, element.width);
    const lines = wrapTextLines(element.text, width, element.fontSize, recordedMeasure);
    const height = lines.length * element.fontSize * TEXT_LINE_HEIGHT_RATIO;
    const content = { x: element.x, y: element.y, width, height };
    const rows = lines.map((text, index) => {
      const advance = recordedMeasure(text || ' ');
      const offset = element.align === 'center' ? (width - advance) / 2
        : element.align === 'right' ? width - advance : 0;
      return { text, advance, x: element.x + offset,
        y: element.y + index * element.fontSize * TEXT_LINE_HEIGHT_RATIO };
    });
    let plate = null;
    if (textHasBackgroundPlate(element)) {
      const pad = textBackgroundPad(element.fontSize);
      const bounds = { x: element.x - pad.x, y: element.y - pad.y,
        width: width + 2 * pad.x, height: height + 2 * pad.y };
      plate = { bounds, radius: textBackgroundRadius(element, bounds.width, bounds.height) };
    }
    const fitted = fitAutoWidthTextElement(element, recordedMeasure);
    const editing = fitEditingAutoWidthTextElement(element, recordedMeasure);
    return { element, measurements, expected: { rows, content, plate }, fitted, editing };
  });
}

if (process.argv.includes('--write')) {
  await writeFile(fixture, `${JSON.stringify(cases(), null, 2)}\n`);
} else {
  test('native paragraph vectors match shipping text helpers', async () => {
    assert.deepEqual(JSON.parse(await readFile(fixture, 'utf8')), cases());
  });
  test('paragraph vectors distinguish wrap thresholds, Unicode and anchor tolerance', () => {
    const data = cases();
    assert.deepEqual(data[1].expected.rows.map(row => row.text), ['A B']);
    assert.deepEqual(data[2].expected.rows.map(row => row.text), ['A', 'B']);
    assert.deepEqual(data[3].expected.rows.map(row => row.text), ['AB', '😀C', 'fi']);
    assert.equal(data[14].fitted.x, -13.25);
    assert.equal(data[15].fitted.x, -12.75);
    assert.equal(data[16].editing.width, 188);
    assert.notEqual(data[17].editing.width, 188);
  });
}
