import { Terminal } from '@xterm/xterm';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { installSpikeCapture, logSpikeData } from './spikeCapture';

// SPIKE(§65.5): Task 4 で除去する
//
// このファイルは `@xterm/xterm` を `vi.mock` しない（契約 §65 の T4 と同じ理由）。
// ここが証明するのは「配線と読み取り経路が実際に動くこと」だけである。
// WKWebView 実機のイベント順序（§65.4 の未決点）はここでは再現できない —— それは
// `npm run tauri dev` 上の手動 spike（§65.5）でしか観測できない。

function logTextareaValue(): string {
  const el = document.getElementById('kamux-spike-log');
  if (!(el instanceof HTMLTextAreaElement)) {
    throw new Error('spike overlay のログ textarea (#kamux-spike-log) が見つからない');
  }
  return el.value;
}

function parsedLogLines(): Array<Record<string, unknown>> {
  return logTextareaValue()
    .split('\n')
    .filter((line) => line.trim().startsWith('{'))
    .map((line) => JSON.parse(line) as Record<string, unknown>);
}

describe('spikeCapture（SPIKE §65.5: Task 4 で除去する）', () => {
  let inputEl: HTMLTextAreaElement;

  beforeEach(() => {
    inputEl = document.createElement('textarea');
    document.body.appendChild(inputEl);
  });

  afterEach(() => {
    inputEl.remove();
    // installSpikeCapture が document.body に足したオーバーレイを毎テスト掃除する
    document.getElementById('kamux-spike-overlay')?.remove();
  });

  it('keydown と input の 2 行を記録する（term.open() 前で term.textarea が undefined でも絞り込みが効かず記録できる）', () => {
    const term = new Terminal();

    installSpikeCapture(term);

    inputEl.dispatchEvent(new KeyboardEvent('keydown', { key: 'Shift', bubbles: true }));
    inputEl.dispatchEvent(
      new InputEvent('input', {
        data: '!',
        inputType: 'insertText',
        bubbles: true,
        composed: true,
      }),
    );

    const rows = parsedLogLines();
    const types = rows.map((r) => r.type);
    expect(types).toContain('keydown');
    expect(types).toContain('input');
  });

  it('_keyDownSeen の欄は null/undefined ではなく boolean である（term._core 経由の読み取りが効いている）', () => {
    const term = new Terminal();

    installSpikeCapture(term);

    inputEl.dispatchEvent(new KeyboardEvent('keydown', { key: 'Shift', bubbles: true }));

    const rows = parsedLogLines();
    const keydownRow = rows.find((r) => r.type === 'keydown');
    expect(keydownRow).toBeDefined();
    expect(typeof keydownRow?._keyDownSeen).toBe('boolean');
  });

  it('logSpikeData を呼ぶと type: "onData" の行が同じタイムラインに入る', () => {
    const term = new Terminal();

    installSpikeCapture(term);
    logSpikeData('s1:agent', 'hello');

    const rows = parsedLogLines();
    const onDataRow = rows.find((r) => r.type === 'onData');
    expect(onDataRow).toBeDefined();
    expect(onDataRow?.data).toBe('hello');
  });
});
