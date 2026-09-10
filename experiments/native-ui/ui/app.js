const { invoke } = window.__TAURI__.core;
const capture = document.querySelector('#capture');
const save = document.querySelector('#save');
const preview = document.querySelector('#preview');
const status = document.querySelector('#status');

capture.addEventListener('click', async () => {
  capture.disabled = save.disabled = true;
  status.textContent = 'Capturing…';
  try {
    const frame = await invoke('capture');
    preview.src = frame.url;
    await preview.decode();
    preview.hidden = false;
    status.textContent = `${frame.width} × ${frame.height} · capture + PNG ${frame.capture_encode_ms.toFixed(1)} ms`;
    save.disabled = false;
  } catch (error) {
    status.textContent = `Error: ${error}`;
  } finally {
    capture.disabled = false;
  }
});
save.addEventListener('click', async () => {
  try { status.textContent = await invoke('save'); }
  catch (error) { status.textContent = `Error: ${error}`; }
});
requestAnimationFrame(() => requestAnimationFrame(() => invoke('ready')));
