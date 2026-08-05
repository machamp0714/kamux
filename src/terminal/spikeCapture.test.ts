import { Terminal } from '@xterm/xterm';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

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
    // レビュー指摘 I1: toContain だけでは「installSpikeCapture を呼ぶたびに document へ
    // 8 本ずつリスナを重ねる」旧実装（1 打鍵が重複記録される）を弁別できない。行数を固定する。
    expect(rows).toHaveLength(2);
  });

  it('2 つの term で installSpikeCapture を呼んでから 1 イベント撃っても記録は 1 行のまま（レビュー指摘 I1: document へのリスナ登録はモジュール単位で 1 度だけ、という修正本体を固定する）', () => {
    const termA = new Terminal();
    const termB = new Terminal();

    installSpikeCapture(termA);
    installSpikeCapture(termB);

    inputEl.dispatchEvent(new KeyboardEvent('keydown', { key: 'Shift', bubbles: true }));

    const rows = parsedLogLines();
    const keydownRows = rows.filter((r) => r.type === 'keydown');
    expect(keydownRows).toHaveLength(1);
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

  it('navigator.clipboard が無い環境でもコピーボタンは select() フォールバックに落ちる（レビュー指摘 I2: writeText の参照自体が同期的に throw するとフォールバックへ到達できない）', () => {
    const term = new Terminal();
    installSpikeCapture(term);

    const originalClipboard = navigator.clipboard;
    // WKWebView 等 clipboard API が存在しない環境を模す
    Object.defineProperty(navigator, 'clipboard', {
      value: undefined,
      configurable: true,
    });

    try {
      const logEl = document.getElementById('kamux-spike-log');
      if (!(logEl instanceof HTMLTextAreaElement)) {
        throw new Error('spike overlay のログ textarea (#kamux-spike-log) が見つからない');
      }
      const selectSpy = vi.spyOn(logEl, 'select');

      const copyButton = document.getElementById('kamux-spike-copy');
      if (!(copyButton instanceof HTMLButtonElement)) {
        throw new Error('spike overlay のコピーボタン (#kamux-spike-copy) が見つからない');
      }
      copyButton.dispatchEvent(new MouseEvent('click', { bubbles: true }));

      expect(selectSpy).toHaveBeenCalled();
    } finally {
      Object.defineProperty(navigator, 'clipboard', {
        value: originalClipboard,
        configurable: true,
      });
    }
  });

  it('dropped 行が 0 本でも、コントロール NG にはフォーカス確認の文言が出る（advisor 再指摘: installedTerms に open() 前の term が残っていると pickTerm 規則2でその term に誤帰属し dropped 行を作らないため、dropped 件数で focus ヒントの出し分けをしてはならない）', async () => {
    vi.resetModules();
    const fresh = await import('./spikeCapture');

    const matchedTextarea = document.createElement('textarea');
    document.body.appendChild(matchedTextarea);
    const openedTerm = { textarea: matchedTextarea } as unknown as Terminal;
    // open() されていない term（textarea 無し）。dispose で剪定されない既知の Minor により、
    // こういう term が installedTerms に残ったまま実機で人間が打鍵する状況が起こりうる。
    const staleUnopenedTerm = new Terminal();

    fresh.installSpikeCapture(openedTerm);
    fresh.installSpikeCapture(staleUnopenedTerm);

    // openedTerm.textarea とは異なる要素へディスパッチ。pickTerm 規則2は「textarea 無しの
    // term」を見つけてしまう（staleUnopenedTerm）ため、rule3（drop）には到達しない。
    inputEl.dispatchEvent(new KeyboardEvent('keydown', { key: 'a', bubbles: true }));

    const rows = parsedLogLines();
    expect(rows.filter((r) => r.type === 'dropped')).toHaveLength(0);
    expect(rows.filter((r) => r.type === 'keydown')).toHaveLength(1);

    const nextButton = document.getElementById('kamux-spike-next');
    if (!(nextButton instanceof HTMLButtonElement)) {
      throw new Error('spike overlay の次のステップへボタン (#kamux-spike-next) が見つからない');
    }
    nextButton.dispatchEvent(new MouseEvent('click', { bubbles: true }));

    const controlEl = document.getElementById('kamux-spike-control');
    if (!(controlEl instanceof HTMLDivElement)) {
      throw new Error('spike overlay のコントロール判定行 (#kamux-spike-control) が見つからない');
    }
    expect(controlEl.textContent).toContain('コントロール NG');
    // dropped 行が 0 本（droppedCount === 0）でも focus ヒントは出る、が固定したい本体
    expect(controlEl.textContent).toContain('フォーカスがあるか確認');
    expect(controlEl.textContent).toContain('やり直してください');

    matchedTextarea.remove();
  });

  // ⚠️ このテストはファイル内の**最後**に置くこと。vi.resetModules() + 動的 import で
  // 作った fresh module の document リスナーは後始末されず残り続ける（installSpikeCapture に
  // teardown が無い、既知の Minor）。このテストの後にさらに it を足すと、その新しいテストの
  // 描画結果を「今から古い fresh module の残存リスナーが誤って触る」可能性がゼロとは言えない
  // ため、順序を変えない。
  it('どの term にもマッチしないイベントは dropped 行として記録され、コントロール NG の理由にフォーカス確認を促す文言が出る（レビュー指摘 I3: 計装が壊れているのかフォーカス漏れなのかを切り分け可能にする）', async () => {
    // installedTerms はモジュール単位の状態で dispose 時に剪定されない（既知の Minor。今回は
    // 対応しない）。他テストで登録済みの textarea 無し term が残っていると pickTerm の規則2が
    // 常にフォールバックしてしまい drop シナリオを再現できないため、モジュールを 1 度だけ
    // リセットしてこのテスト専用のクリーンな installedTerms を作る。
    vi.resetModules();
    const fresh = await import('./spikeCapture');

    const matchedTextarea = document.createElement('textarea');
    document.body.appendChild(matchedTextarea);

    // 実物の Terminal.textarea は open() しないと生成されない。pickTerm が見るのは
    // `.textarea` プロパティの有無だけなので、「open() 済み term」をダミーオブジェクトで模す。
    const openedTerm = { textarea: matchedTextarea } as unknown as Terminal;

    fresh.installSpikeCapture(openedTerm);

    // openedTerm.textarea とは異なる要素へディスパッチ → フォーカスが無い状態を模す。
    // installedTerms には textarea 無し term が他に無いため、pickTerm はどの規則にも
    // 合致せず「捨てる」はずである。
    inputEl.dispatchEvent(new KeyboardEvent('keydown', { key: 'a', bubbles: true }));

    const rows = parsedLogLines();
    const droppedRows = rows.filter((r) => r.type === 'dropped');
    expect(droppedRows).toHaveLength(1);
    expect(droppedRows[0]?.code).toBe('keydown');

    const nextButton = document.getElementById('kamux-spike-next');
    if (!(nextButton instanceof HTMLButtonElement)) {
      throw new Error('spike overlay の次のステップへボタン (#kamux-spike-next) が見つからない');
    }
    // ステップ 1 → 2 に進めてコントロール判定を発火させる（input 行が 1 本も無い状態）
    nextButton.dispatchEvent(new MouseEvent('click', { bubbles: true }));

    const controlEl = document.getElementById('kamux-spike-control');
    if (!(controlEl instanceof HTMLDivElement)) {
      throw new Error('spike overlay のコントロール判定行 (#kamux-spike-control) が見つからない');
    }
    expect(controlEl.textContent).toContain('コントロール NG');
    expect(controlEl.textContent).toContain('フォーカスがあるか確認');
    // advisor 再指摘: NG は常に「まずやり直す」を案内し、いきなり中止させない
    expect(controlEl.textContent).toContain('やり直してください');

    matchedTextarea.remove();
  });
});
