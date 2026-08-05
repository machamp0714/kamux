import { Terminal } from '@xterm/xterm';
import { describe, expect, it } from 'vitest';

// 契約 §65.6 の T4（カナリア）。B / C / D と違い、これは spike の使い捨てコードではなく
// 恒久的な回帰の見張りである（Task 4 で除去しない）。
//
// このファイルは `@xterm/xterm` を `vi.mock` しない（契約 §65.6 の ⚠️）。
// `src/terminal/registry.test.ts` は 115 行目で `Terminal` をフェイクに差し替えており、
// あちらに T4 を置くと自分の偽物を検査するだけになり、カナリアは永遠に赤くならない。

/**
 * 実体で確認済みの経路（契約 §65.2 / lane-controller の裁定）:
 * `_keyDownSeen` / `_keyPressHandled` は public な Terminal ではなく `terminal._core` にある。
 * 契約 §65.12 の「偽 term に `_keyDownSeen` を持たせる」という書き方は経路としては誤りで、
 * 実体は `_core` の下である。このカナリアと今後の手当ては `term._core._keyDownSeen` を正とする。
 */
interface XtermCoreProbe {
  _keyDownSeen?: unknown;
  _keyPressHandled?: unknown;
}

interface TerminalWithCore {
  _core: XtermCoreProbe;
}

describe('契約 §65 のカナリア（T4）: @xterm/xterm の private フィールド名が実体と一致していること', () => {
  it(
    '契約 §65: term._core に _keyDownSeen / _keyPressHandled が boolean で存在する。' +
      'このテストが緑であることは IME の不具合が直っていることを一切意味しない。' +
      '見ているのはフィールド名の存在だけである（契約 §65.6）',
    () => {
      const term = new Terminal();
      const core = (term as unknown as TerminalWithCore)._core;

      expect(
        core,
        '契約 §65: term._core が存在しない。@xterm/xterm の内部構造が変わった可能性がある',
      ).toBeDefined();
      expect(
        typeof core._keyDownSeen,
        '契約 §65: term._core._keyDownSeen が boolean ではない。版が上がってフィールド名が変わった可能性がある',
      ).toBe('boolean');
      expect(
        typeof core._keyPressHandled,
        '契約 §65: term._core._keyPressHandled が boolean ではない。版が上がってフィールド名が変わった可能性がある',
      ).toBe('boolean');
    },
  );
});
