import '../../../apps/desktop/ui/src/styles/mini-preview.css';
import './fixture.css';
import { playThumbnailDustCanvas, playThumbnailDustAnimations } from '../../../apps/desktop/ui/src/lib/thumbnailExit.ts';

const invoke = window.__TAURI_INTERNALS__?.invoke;
const params = new URLSearchParams(location.search);
const config = invoke ? await invoke('configuration')
  : await fetch(`/${params.get('scenario') || 'dust-bottom-left'}.json`).then(r => r.json());
if (params.has('ms')) config.checkpointMs = Number(params.get('ms'));
const stage = document.querySelector('#stage');
const image = new Image();
image.src = '/fixture.png';
await image.decode();
let stop = () => {}, canvasFrame, animations = [], setupMs = [], renderer;
let runGeneration = 0;
const isDust = config.scenario.startsWith('dust');
const bottom = config.scenario.includes('bottom');

function card(y) {
  const article = document.createElement('article');
  article.className = 'thumbnail-card fixture-card';
  article.style.top = `${y}px`;
  const media = document.createElement('div');
  media.className = 'thumbnail-media';
  media.append(image.cloneNode());
  article.append(media); stage.append(article);
  return article;
}

function prepare(live = false) {
  stop(); for (const animation of animations) animation.cancel();
  canvasFrame = undefined; animations = []; stage.replaceChildren();
  const survivor = card(bottom ? 136 : 504);
  // Use the production translate transition and get its actual CSS easing.
  survivor.style.translate = '0 0';
  survivor.getBoundingClientRect();
  survivor.classList.add('thumbnail-stack-shifting');
  if (live && isDust) survivor.style.transitionDelay = '1800ms';
  survivor.style.setProperty('--thumbnail-stack-shift', `${bottom ? 184 : -184}px`);
  survivor.style.removeProperty('translate');
  survivor.getBoundingClientRect();
  const settle = survivor.getAnimations().find(a => a.transitionProperty === 'translate');
  if (!settle) throw new Error('Shipping stack-settle transition was not created');
  if (!live) { settle.pause(); settle.currentTime = 0; }
  // The production controller starts this transition at 1800ms for dust.
  animations.push({ set currentTime(t) { settle.currentTime = Math.max(0, t - (isDust ? 1800 : 0)); }, cancel: () => settle.cancel() });
  if (isDust) {
    const exiting = card(320);
    exiting.classList.add('thumbnail-exit-delete', 'thumbnail-exit-dust', 'thumbnail-exiting');
    exiting.querySelector('img').classList.add('thumbnail-dust-source');
    const layer = document.createElement('div');
    layer.className = 'thumbnail-dust-layer';
    const canvas = document.createElement('canvas'); canvas.className = 'thumbnail-dust-canvas';
    layer.append(canvas); exiting.append(layer);
    stop = playThumbnailDustCanvas(canvas, image, config.particles, live ? {} : {
      now: () => 0, frame: callback => { canvasFrame = callback; return 1; }, cancelFrame: () => { canvasFrame = undefined; },
    });
    renderer = 'tauri-canvas';
    if (!stop) {
      canvas.remove(); renderer = 'tauri-dom-waapi';
      for (const p of config.particles) {
        const chip = document.createElement('span'); chip.className = 'thumbnail-dust';
        Object.assign(chip.style, { left: `${p.left}px`, top: `${p.top}px`, width: `${p.width}px`, height: `${p.height}px` });
        const surface = document.createElement('span'); surface.className = 'thumbnail-dust-surface';
        Object.assign(surface.style, { left: `${-p.sourceLeft}px`, top: `${-p.sourceTop}px`, width: `${p.cardWidth}px`, height: `${p.cardHeight}px`,
          backgroundImage: 'url(/fixture.png)', backgroundSize: `${p.surfaceWidth}px ${p.surfaceHeight}px`, backgroundPosition: `${p.surfaceOffsetX}px ${p.surfaceOffsetY}px` });
        chip.append(surface); layer.append(chip);
      }
      stop = playThumbnailDustAnimations(layer.children, config.particles);
    }
    for (const animation of exiting.getAnimations({ subtree: true })) {
      if (!live) { animation.pause(); animation.currentTime = 0; }
      animations.push(animation);
    }
  } else { renderer = 'tauri-css-settle'; stop = () => {}; }
}

function pose(ms) {
  if (canvasFrame) { const callback = canvasFrame; canvasFrame = undefined; callback(ms); }
  for (const animation of animations) animation.currentTime = ms;
}
const frame = () => new Promise(resolve => requestAnimationFrame(resolve));
const metadata = () => ({ schema: 1, scale: devicePixelRatio, scenario: config.scenario, mode: config.mode, renderer,
  checkpointMs: config.checkpointMs, width: innerWidth, height: innerHeight });

async function checkpoint(ms) {
  runGeneration++; prepare(); pose(ms); await frame(); await frame();
  window.parity.ready = true;
  return metadata();
}

async function run(durationMs = config.durationMs) {
  const generation = ++runGeneration;
  window.parity.ready = false;
  setupMs = []; const intervals = []; let previous, cycle = -1;
  const start = performance.now();
  let elapsed = 0;
  while (generation === runGeneration && elapsed < durationMs) {
    await frame();
    // Main-thread callback arrival, not rAF's scheduled timestamp or presentation.
    const now = performance.now(); elapsed = now - start;
    if (previous !== undefined) intervals.push(now - previous);
    previous = now;
    if (elapsed >= durationMs) break;
    const nextCycle = Math.floor(elapsed / config.cycleMs);
    if (nextCycle !== cycle) {
      cycle = nextCycle;
      const began = performance.now(); prepare(true); setupMs.push(performance.now() - began);
    }
    // In live trials CSS/WAAPI stay compositor-driven and Canvas uses its real
    // production rAF loop. Only checkpoints pause/seek the animations.
  }
  if (generation !== runGeneration) return;
  const result = { ...metadata(), elapsedMs: elapsed, callbackIntervalsMs: intervals, setupMs, cycles: setupMs.length,
    complete: true, metric: 'main-thread animation callback intervals; not GPU presented frames' };
  window.parity.result = result;
  if (invoke) await invoke('record', { kind: 'result', value: result });
  return result;
}

window.parity = { ready: false, checkpoint, run, config };
try {
  const info = await checkpoint(config.checkpointMs);
  if (invoke) {
    if (devicePixelRatio !== config.scale || innerWidth !== config.width || innerHeight !== config.height) {
      throw new Error(`Viewport mismatch: ${JSON.stringify(info)}`);
    }
    await invoke('record', { kind: 'ready', value: info });
    if (config.mode === 'run') await run();
  }
} catch (error) {
  window.parity.error = String(error);
  if (invoke) await invoke('record', { kind: 'error', value: { error: String(error) } });
  else console.error(error);
}
