import { mkdir, writeFile } from 'node:fs/promises';
import { deflateSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { buildThumbnailDustParticles, thumbnailDustVisualAt } from '../../apps/desktop/ui/src/lib/thumbnailExit.ts';

export const root = fileURLToPath(new URL('.', import.meta.url));
export const scenarios = ['dust-bottom-left', 'dust-top-right', 'settle-bottom', 'settle-top'];
export function random(seed) {
  return () => ((seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0) / 2 ** 32);
}

// One lossless, asymmetric RGB fixture, decoded independently by each renderer.
// Its aspect ratio deliberately differs from the card's to exercise cover crop.
function fixturePNG() {
  const width = 397, height = 251;
  const bytes = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      let color = [25 + Math.floor(75 * x / width), 50 + Math.floor(95 * y / height), 105 + Math.floor(65 * x / width)];
      if (y > 180 - x * .22) color = [40, 105 + Math.floor(40 * x / width), 98];
      if ((x - 303) ** 2 + (y - 61) ** 2 < 30 ** 2) color = [244, 186, 74];
      if (x > 47 && x < 138 && y > 43 && y < 147) color = [216, 85, 105];
      if (x > 61 && x < 113 && y > 58 && y < 121) color = [242, 227, 206];
      if (x > 174 && x < 272 && y > 128 && y < 158) color = [105, 76, 168];
      if ((x % 31 < 2 || y % 37 < 2) && x < 170 && y > 162) color = [216, 226, 231];
      const offset = y * (width * 3 + 1) + 1 + x * 3;
      bytes.set(color, offset);
    }
  }
  function chunk(type, data) {
    const body = Buffer.concat([Buffer.from(type), data]);
    let crc = 0xffffffff;
    for (const byte of body) {
      crc ^= byte;
      for (let n = 0; n < 8; n++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
    }
    const size = Buffer.alloc(4), end = Buffer.alloc(4);
    size.writeUInt32BE(data.length); end.writeUInt32BE((crc ^ 0xffffffff) >>> 0);
    return Buffer.concat([size, body, end]);
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', header), chunk('sRGB', Buffer.from([0])), chunk('IDAT', deflateSync(bytes)), chunk('IEND', Buffer.alloc(0))]);
}

export function config(scenario) {
  if (!scenarios.includes(scenario)) throw new Error(`Unknown scenario ${scenario}`);
  return {
    schema: 1, scenario, mode: 'checkpoint', checkpointMs: 420, durationMs: 12800,
    cycleMs: 3200, width: 640, height: 720, scale: 2,
    fixturePath: `${root}.build/fixture.png`, seed: 739,
    particles: buildThumbnailDustParticles(284, 160, {
      random: random(739), originX: scenario.endsWith('right') ? 226.5 : 22.5,
      originY: 22.5, imageWidth: 397, imageHeight: 251,
    }),
  };
}

await mkdir(`${root}.build/public`, { recursive: true });
await writeFile(`${root}.build/fixture.png`, fixturePNG());
await writeFile(`${root}.build/public/fixture.png`, fixturePNG());
for (const scenario of scenarios) {
  await writeFile(`${root}.build/public/${scenario}.json`, JSON.stringify(config(scenario)));
}
// Include individual delay boundaries and uneven timestamps, not only end poses.
const particles = [...config(scenarios[0]).particles, ...config(scenarios[1]).particles];
const times = [...new Set([-1, 0, 1, 110, 204, 420, 777, 900, 1307, 1800, 2090, 2379, 2550,
  ...particles.flatMap(p => [p.delayMs - .01, p.delayMs, p.delayMs + .01])])];
await writeFile(`${root}.build/poses-input.json`, JSON.stringify({ particles, times }));
await writeFile(`${root}.build/poses-expected.json`, JSON.stringify(times.map(t => particles.map(p => thumbnailDustVisualAt(p, t)))));
