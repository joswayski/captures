import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
const expected = JSON.parse(await readFile(new URL('.build/poses-expected.json', import.meta.url)));
const actual = JSON.parse(await readFile(process.argv[2]));
assert.equal(actual.length, expected.length);
let count = 0, maximumError = 0;
for (let t = 0; t < expected.length; t++) {
  assert.equal(actual[t].length, expected[t].length);
  for (let p = 0; p < expected[t].length; p++) {
    for (const key of ['opacity', 'dx', 'dy', 'rotate', 'scale']) {
      const error = Math.abs(expected[t][p][key] - actual[t][p][key]);
      assert.ok(Number.isFinite(error) && error < 1e-9, `time ${t} particle ${p} ${key}: error ${error}`);
      maximumError = Math.max(maximumError, error); count++;
    }
  }
}
console.log(`${count} Swift-vs-shipping-TS scalar comparisons passed; max error ${maximumError}`);
