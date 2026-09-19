import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import test from 'node:test';
import { recordShortcut, shortcutDisplayTokens } from '../apps/desktop/ui/src/lib/shortcut.ts';

const fixture = new URL('../crates/captures-app/tests/shortcut-golden.json', import.meta.url);
const platforms = ['macos', 'windows', 'linux'];
// Include every named key, with asymmetric modifier combinations. Boundary
// cases distinguish physical codes from characters, function/keypad limits,
// bare Print Screen from other keys, and modifier-only input from completion.
const named = [
  'AudioVolumeDown', 'AudioVolumeMute', 'AudioVolumeUp', 'Backquote', 'Backslash',
  'Backspace', 'BracketLeft', 'BracketRight', 'CapsLock', 'Comma', 'Delete', 'End',
  'Enter', 'Equal', 'Home', 'Insert', 'MediaPause', 'MediaPlay', 'MediaPlayPause',
  'MediaStop', 'MediaTrackNext', 'MediaTrackPrevious', 'Minus', 'NumLock', 'NumpadAdd',
  'NumpadDecimal', 'NumpadDivide', 'NumpadEnter', 'NumpadEqual', 'NumpadMultiply',
  'NumpadSubtract', 'PageDown', 'PageUp', 'Pause', 'Period', 'PrintScreen', 'Quote',
  'ScrollLock', 'Semicolon', 'Slash', 'Space', 'Tab', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'ArrowUp',
];
const boundary = ['Escape', 'PrintScreen', 'KeyA', 'KeyZ', 'keyA', 'Keya', 'KeyÄ', 'A',
  'Digit0', 'Digit9', 'Digit10', 'Numpad0', 'Numpad9', 'Numpad10', 'F0', 'F1', 'F24', 'F25',
  'F01', 'F+1', 'F999', '', 'Unidentified', 'Fn', 'ControlLeft', 'ControlRight',
  'ShiftLeft', 'ShiftRight', 'AltLeft', 'AltRight', 'MetaLeft', 'MetaRight', 'OSLeft', 'OSRight'];
const event = (code, mask) => ({ code, ctrlKey: Boolean(mask & 1), shiftKey: Boolean(mask & 2),
  altKey: Boolean(mask & 4), metaKey: Boolean(mask & 8) });

function shippingCases() {
  const recording = platforms.flatMap(platform => {
    const inputs = [
      ...named.map((code, i) => event(code, 1 + i % 15)),
      ...boundary.flatMap(code => [event(code, 0), event(code, 15)]),
      ...Array.from({ length: 16 }, (_, mask) => event('KeyQ', mask)),
    ];
    return inputs.map(event => ({ event, platform, expected: recordShortcut(event, platform) }));
  });
  const labels = [...named, 'Control+Shift+Alt+Super+KeyA', ' Ctrl + option + Digit9 ',
    'CommandOrControl+Enter', 'commandorctrl+Backspace', 'CmdOrCtrl+keyz', 'cmdorcontrol+Numpad9',
    'COMMAND+Meta+super+Shift+KeyQ', 'print', 'PrtScn', 'prtsc', 'PrintScreen', 'backquote',
    'digit0', 'numpad0', 'Numpad10', 'KeyÄ', 'Unknown', '', ' + + '];
  const display = platforms.flatMap(platform => labels.map(shortcut => ({ shortcut, platform,
    expected: shortcutDisplayTokens(shortcut, platform) })));
  return { recording, display };
}

if (process.argv.includes('--write')) {
  const { recording, display } = shippingCases();
  await writeFile(fixture, `{"recording":[\n${recording.map(c => JSON.stringify(c)).join(',\n')}\n],"display":[\n${display.map(c => JSON.stringify(c)).join(',\n')}\n]}\n`);
} else {
  test('native shortcut vectors match the shipping recorder and display oracle', async () => {
    assert.deepEqual(JSON.parse(await readFile(fixture, 'utf8')), shippingCases());
  });
  test('shortcut vectors distinguish cancellation, waiting, invalid input and all modifiers', () => {
    const cases = shippingCases().recording;
    assert.deepEqual(new Set(cases.map(c => c.expected.kind)), new Set(['cancel', 'waiting', 'invalid', 'complete']));
    assert.ok(cases.some(c => c.expected.shortcut === 'Control+Shift+Alt+Super+KeyQ'));
    assert.ok(cases.some(c => c.event.code === 'PrintScreen' && c.expected.shortcut === 'PrintScreen'));
    assert.ok(cases.some(c => c.event.code === 'F25' && c.expected.kind === 'invalid'));
  });
}
