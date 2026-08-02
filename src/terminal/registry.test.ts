import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/**
 * xterm 本体は jsdom で完全には動かないため、契約 §16 の 8 関数の「配線」だけを
 * フェイクの Terminal / アドオンで検証する。実描画は契約 §26 の E2E に回す。
 */

/**
 * このプロジェクトは `@types/node` を依存に持たないため、`process` はグローバルに
 * 型付けされていない（実行環境が vitest = Node なので実体は存在する）。
 * unhandled rejection の実測にだけ使うので、必要な最小限のシグネチャだけを
 * このテストファイルに閉じたアンビエント宣言として与える。
 */
declare const process: {
  on(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
  off(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
};

const { FakeTerminal, FakeFitAddon, FakeWebglAddon, FakeCanvasAddon } = vi.hoisted(() => {
  class FakeTerminal {
    static instances: FakeTerminal[] = [];
    onDataHandlers: Array<(data: string) => void> = [];
    onBinaryHandlers: Array<(data: string) => void> = [];
    writes: Array<{ data: unknown; cb?: () => void }> = [];
    cols = 80;
    rows = 24;
    buffer = { active: { viewportY: 0, baseY: 0 } };
    disposed = false;
    scrollToBottomCalls = 0;
    scrollToLineCalls: number[] = [];

    constructor(public options: unknown) {
      FakeTerminal.instances.push(this);
    }

    loadAddon(): void {
      // 何もしない。呼ばれたことのアサートは不要
    }

    onData(cb: (data: string) => void): void {
      this.onDataHandlers.push(cb);
    }

    onBinary(cb: (data: string) => void): void {
      this.onBinaryHandlers.push(cb);
    }

    customKeyEventHandler: ((event: { metaKey: boolean }) => boolean) | null = null;

    attachCustomKeyEventHandler(handler: (event: { metaKey: boolean }) => boolean): void {
      this.customKeyEventHandler = handler;
    }

    open(): void {
      // jsdom では実描画しない
    }

    focus(): void {
      // 配線の対象外
    }

    write(data: unknown, cb?: () => void): void {
      this.writes.push({ data, cb });
      cb?.();
    }

    scrollToBottom(): void {
      this.scrollToBottomCalls += 1;
    }

    scrollToLine(line: number): void {
      this.scrollToLineCalls.push(line);
    }

    dispose(): void {
      this.disposed = true;
    }
  }

  class FakeFitAddon {
    fit(): void {
      // 配線の対象外
    }
  }

  class FakeWebglAddon {
    static instances: FakeWebglAddon[] = [];
    contextLossHandlers: Array<() => void> = [];
    disposed = false;

    constructor() {
      FakeWebglAddon.instances.push(this);
    }

    onContextLoss(cb: () => void): void {
      this.contextLossHandlers.push(cb);
    }

    dispose(): void {
      this.disposed = true;
    }
  }

  class FakeCanvasAddon {
    disposed = false;

    dispose(): void {
      this.disposed = true;
    }
  }

  return { FakeTerminal, FakeFitAddon, FakeWebglAddon, FakeCanvasAddon };
});

vi.mock('@xterm/xterm', () => ({ Terminal: FakeTerminal }));
vi.mock('@xterm/addon-fit', () => ({ FitAddon: FakeFitAddon }));
vi.mock('@xterm/addon-webgl', () => ({ WebglAddon: FakeWebglAddon }));
vi.mock('@xterm/addon-canvas', () => ({ CanvasAddon: FakeCanvasAddon }));

const writePty = vi.fn().mockResolvedValue(undefined);
const writePtyBytes = vi.fn().mockResolvedValue(undefined);

/**
 * `ackPty` だけは意図的に `vi.fn()` で包まない。
 *
 * vitest の `vi.fn()` は呼び出し結果を `mock.results` に記録するために内部で
 * Promise を横取りしており、それ自体が Node の「unhandled rejection ではない
 * （処理済みである）」判定を成立させてしまう（Task 13 の実測で判明）。
 * `ack_pty` の reject が本当に unhandled にならないかを検証するテスト
 * （下の「ack_pty の reject 処理」describe）が `vi.fn()` 越しだと原理的に
 * 何も検出できなくなるため、素の関数変数として持つ。
 */
const defaultAckPtyImpl = (_surfaceId: string, _seq: number): Promise<void> =>
  Promise.resolve(undefined);
let ackPtyImpl: (surfaceId: string, seq: number) => Promise<void> = defaultAckPtyImpl;

/**
 * Important 2（PR 10 fix round 2）: writePty / writePtyBytes も ack_pty と同じ理由で
 * unhandled rejection の回帰テストを持つ必要があるが、既存の call-args アサーション
 * （`term.onData / onBinary の単一登録` describe）は `vi.fn()` の呼び出し記録に依存
 * している。両立させるため、呼び出し口を「差し替え可能な関数変数」にし、既定では
 * 上の vi.fn() へ委譲する。回帰テストだけ素の reject する関数へ挿げ替える。
 */
const defaultWritePtyImpl = (surfaceId: string, data: string): Promise<void> =>
  writePty(surfaceId, data);
let writePtyImpl: (surfaceId: string, data: string) => Promise<void> = defaultWritePtyImpl;
const defaultWritePtyBytesImpl = (surfaceId: string, base64: string): Promise<void> =>
  writePtyBytes(surfaceId, base64);
let writePtyBytesImpl: (surfaceId: string, base64: string) => Promise<void> =
  defaultWritePtyBytesImpl;

vi.mock('../ipc/commands', () => ({
  writePty: (surfaceId: string, data: string) => writePtyImpl(surfaceId, data),
  writePtyBytes: (surfaceId: string, base64: string) => writePtyBytesImpl(surfaceId, base64),
  ackPty: (surfaceId: string, seq: number) => ackPtyImpl(surfaceId, seq),
}));

const { AckCoalescer } = await import('./ackCoalescer');
const registry = await import('./registry');

function fakeOf(surfaceId: string): InstanceType<typeof FakeTerminal> {
  const term = registry.getTerminal(surfaceId);
  if (!term) throw new Error(`no terminal for ${surfaceId}`);
  return term as unknown as InstanceType<typeof FakeTerminal>;
}

let seq = 0;
/** テストごとに衝突しない surfaceId を作る */
function nextSurfaceId(kind: 'agent' | 'editor' = 'agent'): string {
  seq += 1;
  return `s${seq}:${kind}`;
}

beforeEach(() => {
  FakeTerminal.instances.length = 0;
  // afterEach の vi.restoreAllMocks() は素の vi.fn()（spy 元が無い）に対しては
  // 実装を空関数へ戻す（mockReset 相当）。Important 2（PR 10 fix round 1）で
  // onData/onBinary が writePty(...).catch(...) を呼ぶようになったため、
  // 戻り値が Promise であることを毎テスト再設定する必要がある
  // （でないと 2 テスト目以降で「Cannot read properties of undefined (reading 'catch')」になる）
  writePty.mockReset().mockResolvedValue(undefined);
  writePtyBytes.mockReset().mockResolvedValue(undefined);
  // 回帰テストが挿げ替えても、次のテストでは既定の vi.fn() 経由へ必ず戻す
  writePtyImpl = defaultWritePtyImpl;
  writePtyBytesImpl = defaultWritePtyBytesImpl;
});

afterEach(() => {
  for (const id of registry.listTerminals()) {
    registry.disposeTerminal(id);
  }
  vi.restoreAllMocks();
});

describe('ensureTerminal', () => {
  it('同じ surfaceId には同じインスタンスを返す（冪等）', () => {
    const sid = nextSurfaceId();
    const a = registry.ensureTerminal(sid);
    const b = registry.ensureTerminal(sid);
    expect(a).toBe(b);
    expect(FakeTerminal.instances).toHaveLength(1);
  });
});

describe('term.onData / onBinary の単一登録（契約 §16・必達 2a）', () => {
  it('attachTerminal を複数回呼んでも onData のリスナは 1 本のまま', () => {
    const sid = nextSurfaceId();
    const containerA = document.createElement('div');
    const containerB = document.createElement('div');

    registry.ensureTerminal(sid);
    registry.attachTerminal(sid, containerA);
    registry.detachTerminal(sid);
    registry.attachTerminal(sid, containerB);

    const term = fakeOf(sid);
    expect(term.onDataHandlers).toHaveLength(1);

    // 打鍵 1 回 = 登録された全ハンドラが 1 回ずつ発火する（xterm の実際の挙動）
    term.onDataHandlers.forEach((h) => h('x'));
    expect(writePty).toHaveBeenCalledTimes(1);
    expect(writePty).toHaveBeenCalledWith(sid, 'x');
  });

  it('onBinary のリスナも 1 本のまま', () => {
    const sid = nextSurfaceId();
    const container = document.createElement('div');
    registry.ensureTerminal(sid);
    registry.attachTerminal(sid, container);
    registry.detachTerminal(sid);
    registry.attachTerminal(sid, container);

    const term = fakeOf(sid);
    expect(term.onBinaryHandlers).toHaveLength(1);

    term.onBinaryHandlers.forEach((h) => h('\x01'));
    expect(writePtyBytes).toHaveBeenCalledTimes(1);
    // encodeBinaryString('\x01') の base64。生データのまま渡していないことを締める
    expect(writePtyBytes).toHaveBeenCalledWith(sid, 'AQ==');
  });
});

describe('attachTerminal / detachTerminal', () => {
  it('別のコンテナへ付け替えると host がそちらへ移動する', () => {
    const sid = nextSurfaceId();
    const containerA = document.createElement('div');
    const containerB = document.createElement('div');

    registry.attachTerminal(sid, containerA);
    expect(containerA.children).toHaveLength(1);

    registry.detachTerminal(sid);
    registry.attachTerminal(sid, containerB);

    expect(containerA.children).toHaveLength(0);
    expect(containerB.children).toHaveLength(1);
  });

  it('attachTerminal は void を返す（Promise ではない、契約 §16・必達 2b）', () => {
    const sid = nextSurfaceId();
    const container = document.createElement('div');
    const result = registry.attachTerminal(sid, container);
    expect(result).toBeUndefined();
  });
});

describe('writeToTerminal の seq 後退検知（契約 §16・必達 1）', () => {
  it('seq が後退したら ack.reset() を呼ぶ', () => {
    const sid = nextSurfaceId();
    const resetSpy = vi.spyOn(AckCoalescer.prototype, 'reset');

    registry.writeToTerminal(sid, new Uint8Array([1]), 5);
    registry.writeToTerminal(sid, new Uint8Array([2]), 1);

    expect(resetSpy).toHaveBeenCalledTimes(1);
  });

  it('seq が単調増加している間は ack.reset() を呼ばない', () => {
    const sid = nextSurfaceId();
    const resetSpy = vi.spyOn(AckCoalescer.prototype, 'reset');

    registry.writeToTerminal(sid, new Uint8Array([1]), 1);
    registry.writeToTerminal(sid, new Uint8Array([2]), 2);
    registry.writeToTerminal(sid, new Uint8Array([3]), 3);

    expect(resetSpy).not.toHaveBeenCalled();
  });

  it('term.write のコールバックで ack.consumed(seq) を呼ぶ（ack の一周が閉じている）', () => {
    const sid = nextSurfaceId();
    const consumedSpy = vi.spyOn(AckCoalescer.prototype, 'consumed');

    registry.writeToTerminal(sid, new Uint8Array([9]), 42);

    expect(consumedSpy).toHaveBeenCalledWith(42);
  });
});

describe('ack_pty の reject 処理（契約 §16・必達 1・fix round 1）', () => {
  afterEach(() => {
    // 次のテストに影響しないよう、必ず「解決する」実装へ戻す
    ackPtyImpl = defaultAckPtyImpl;
  });

  it('ack_pty が reject しても unhandled rejection にならない（.catch で握り潰す）', async () => {
    // なぜ vi.fn().mockRejectedValue(...) を使わないか:
    // vitest の vi.fn() は呼び出し結果を mock.results に記録するために内部で
    // Promise を横取りしており、それ自体が Node の「処理済み」判定を成立させてしまう。
    // そのため vi.fn() 経由では unhandled rejection が絶対に観測できない
    // （Task 13 の実測で判明。`ackPtyImpl` がただの関数変数で vi.fn ではないのはこのため。
    // ここを vi.fn() に書き換えるとこのテストは黙って空振りに戻る）。
    ackPtyImpl = (): Promise<void> =>
      Promise.reject(new Error('NotFound: surface already disposed'));

    const rejections: unknown[] = [];
    const onUnhandledRejection = (reason: unknown): void => {
      rejections.push(reason);
    };
    process.on('unhandledRejection', onUnhandledRejection);

    try {
      registry.writeToTerminal(nextSurfaceId(), new Uint8Array([1]), 1);

      // AckCoalescer は queueMicrotask で flush する。unhandledRejection は
      // イベントループを最低 1 周させないと発火しないため、マクロタスクまで進める。
      await new Promise((resolve) => setTimeout(resolve, 0));
      await new Promise((resolve) => setTimeout(resolve, 0));
    } finally {
      process.off('unhandledRejection', onUnhandledRejection);
    }

    expect(rejections).toHaveLength(0);
  });
});

describe('write_pty / write_pty_bytes の reject 処理（契約 §16・Important 2・PR 10 fix round 2）', () => {
  afterEach(() => {
    // 次のテストに影響しないよう、必ず「解決する」実装へ戻す
    writePtyImpl = defaultWritePtyImpl;
    writePtyBytesImpl = defaultWritePtyBytesImpl;
  });

  it('write_pty が reject しても unhandled rejection にならない（.catch で握り潰す）', async () => {
    // ack_pty のテストと同じ理由（コメント参照）で vi.fn() を経由させない
    writePtyImpl = (): Promise<void> =>
      Promise.reject(new Error('NotFound: surface already disposed'));

    const rejections: unknown[] = [];
    const onUnhandledRejection = (reason: unknown): void => {
      rejections.push(reason);
    };
    process.on('unhandledRejection', onUnhandledRejection);

    try {
      const sid = nextSurfaceId();
      registry.ensureTerminal(sid);
      const term = fakeOf(sid);
      term.onDataHandlers.forEach((h) => h('x'));

      await new Promise((resolve) => setTimeout(resolve, 0));
      await new Promise((resolve) => setTimeout(resolve, 0));
    } finally {
      process.off('unhandledRejection', onUnhandledRejection);
    }

    expect(rejections).toHaveLength(0);
  });

  it('write_pty_bytes が reject しても unhandled rejection にならない（.catch で握り潰す）', async () => {
    writePtyBytesImpl = (): Promise<void> =>
      Promise.reject(new Error('NotFound: surface already disposed'));

    const rejections: unknown[] = [];
    const onUnhandledRejection = (reason: unknown): void => {
      rejections.push(reason);
    };
    process.on('unhandledRejection', onUnhandledRejection);

    try {
      const sid = nextSurfaceId();
      registry.ensureTerminal(sid);
      const term = fakeOf(sid);
      term.onBinaryHandlers.forEach((h) => h('\x01'));

      await new Promise((resolve) => setTimeout(resolve, 0));
      await new Promise((resolve) => setTimeout(resolve, 0));
    } finally {
      process.off('unhandledRejection', onUnhandledRejection);
    }

    expect(rejections).toHaveLength(0);
  });
});

describe('loadAcceleratedRenderer との配線', () => {
  it('WebGL が採用されたら onContextLoss を登録する', () => {
    const sid = nextSurfaceId();
    const container = document.createElement('div');
    FakeWebglAddon.instances.length = 0;

    registry.attachTerminal(sid, container);

    expect(FakeWebglAddon.instances).toHaveLength(1);
    expect(FakeWebglAddon.instances[0]?.contextLossHandlers).toHaveLength(1);
  });

  it(
    'onContextLoss が発火すると WebGL アドオンを実際に dispose して降格させる ' +
      '（計画 §2.4。ハンドラを noop に差し替えただけでは緑にならない形）',
    () => {
      const sid = nextSurfaceId();
      const container = document.createElement('div');
      FakeWebglAddon.instances.length = 0;

      registry.attachTerminal(sid, container);
      const addon = FakeWebglAddon.instances[0];
      expect(addon?.disposed).toBe(false);

      // WebGL コンテキストロストを模擬発火（本物の xterm は GPU プロセスクラッシュ等で呼ぶ）
      addon?.contextLossHandlers.forEach((h) => h());

      expect(addon?.disposed).toBe(true);
    },
  );
});

describe('契約規定の配線（fix round 1 で追加）', () => {
  it('scrollbackFor: :editor 接尾辞ならスクロールバックを 1,000 行にする（契約 §19）', () => {
    const sid = nextSurfaceId('editor');
    registry.ensureTerminal(sid);
    const term = fakeOf(sid);
    const options = term.options as { scrollback?: number };
    expect(options.scrollback).toBe(1_000);
  });

  it('scrollbackFor: エージェント用サーフェスは 10,000 行のまま', () => {
    const sid = nextSurfaceId('agent');
    registry.ensureTerminal(sid);
    const term = fakeOf(sid);
    const options = term.options as { scrollback?: number };
    expect(options.scrollback).toBe(10_000);
  });

  it('cursorBlink は false で生成する（契約 §0: アイドル CPU をほぼ 0% に保つ）', () => {
    const sid = nextSurfaceId();
    registry.ensureTerminal(sid);
    const term = fakeOf(sid);
    const options = term.options as { cursorBlink?: boolean };
    expect(options.cursorBlink).toBe(false);
  });

  it('attachCustomKeyEventHandler で Cmd 系キーを xterm に処理させない（契約 §11）', () => {
    const sid = nextSurfaceId();
    registry.ensureTerminal(sid);
    const term = fakeOf(sid);
    expect(term.customKeyEventHandler).not.toBeNull();
    expect(term.customKeyEventHandler?.({ metaKey: true })).toBe(false);
    expect(term.customKeyEventHandler?.({ metaKey: false })).toBe(true);
  });

  it('detachTerminal は WebGL アドオンを dispose して DOM レンダラへ降格する（計画 §2.4）', () => {
    const sid = nextSurfaceId();
    const container = document.createElement('div');
    FakeWebglAddon.instances.length = 0;

    registry.attachTerminal(sid, container);
    const addon = FakeWebglAddon.instances[0];
    expect(addon?.disposed).toBe(false);

    registry.detachTerminal(sid);

    expect(addon?.disposed).toBe(true);
  });
});

describe('readTerminalFont（RULINGS §25: lineHeight は渡さない。§23.2 は撤回済み）', () => {
  afterEach(() => {
    document.documentElement.style.removeProperty('--font-mono');
    document.documentElement.style.removeProperty('--text-sm');
    document.documentElement.style.removeProperty('--leading-term');
  });

  it('--leading-term トークンが読めても lineHeight は渡さない（xterm の単位が CSS の line-height と違うため）', () => {
    // トークンを実際に注入したうえで「それでも渡さない」ことを見る。
    // トークン注入なしでは --leading-term は常に空文字（NaN）を返すため、
    // 実装のあらゆる変異が緑になる no-op テストになってしまう。
    document.documentElement.style.setProperty('--leading-term', '1.65');
    const sid = nextSurfaceId();
    registry.ensureTerminal(sid);
    const term = fakeOf(sid);
    const options = term.options as { lineHeight?: number };
    expect(options.lineHeight).toBeUndefined();
  });

  it('--font-mono / --text-sm もそのまま fontFamily / fontSize に渡す', () => {
    document.documentElement.style.setProperty('--font-mono', 'JetBrains Mono, monospace');
    document.documentElement.style.setProperty('--text-sm', '12px');
    const sid = nextSurfaceId();
    registry.ensureTerminal(sid);
    const term = fakeOf(sid);
    const options = term.options as { fontFamily?: string; fontSize?: number };
    expect(options.fontFamily).toBe('JetBrains Mono, monospace');
    expect(options.fontSize).toBe(12);
  });

  it('トークンが読めない（NaN / 空文字）ときはキー自体を省略し xterm の既定値に委ねる', () => {
    const sid = nextSurfaceId();
    registry.ensureTerminal(sid);
    const term = fakeOf(sid);
    const options = term.options as {
      fontFamily?: string;
      fontSize?: number;
      lineHeight?: number;
    };
    expect(options.fontFamily).toBeUndefined();
    expect(options.fontSize).toBeUndefined();
    expect(options.lineHeight).toBeUndefined();
  });
});

describe('invalidateFitCache（fix round 1・PTY 再起動時の fit キャッシュ無効化）', () => {
  it('無効化後は寸法が前回と同じでも fitTerminal が null を返さない', () => {
    const sid = nextSurfaceId();
    const container = document.createElement('div');
    registry.attachTerminal(sid, container);
    const host = container.firstElementChild as HTMLElement;
    vi.spyOn(host, 'getBoundingClientRect').mockReturnValue({
      width: 800,
      height: 600,
    } as unknown as DOMRect);

    expect(registry.fitTerminal(sid)).toEqual({ cols: 80, rows: 24 });
    // キャッシュにより 2 回目は null（寸法が変わっていない）
    expect(registry.fitTerminal(sid)).toBeNull();

    registry.invalidateFitCache(sid);

    // 無効化後は寸法が同じでも null を返さない（PTY 再起動後に resize_pty を必ず送るため）
    expect(registry.fitTerminal(sid)).toEqual({ cols: 80, rows: 24 });
  });

  it('存在しない surfaceId に対しては何もしない（例外を投げない）', () => {
    expect(() => registry.invalidateFitCache('no-such-surface:agent')).not.toThrow();
  });
});

describe('fitTerminal', () => {
  it('未 attach なら null', () => {
    const sid = nextSurfaceId();
    registry.ensureTerminal(sid);
    expect(registry.fitTerminal(sid)).toBeNull();
  });

  it('0x0 の間は null を返す（0 列 0 行を PTY に送らない）', () => {
    const sid = nextSurfaceId();
    const container = document.createElement('div');
    registry.attachTerminal(sid, container);
    // jsdom の getBoundingClientRect は既定で 0x0
    expect(registry.fitTerminal(sid)).toBeNull();
  });

  it('寸法があれば cols/rows を返し、変化が無ければ 2 回目は null', () => {
    const sid = nextSurfaceId();
    const container = document.createElement('div');
    registry.attachTerminal(sid, container);
    const host = container.firstElementChild as HTMLElement;
    vi.spyOn(host, 'getBoundingClientRect').mockReturnValue({
      width: 800,
      height: 600,
    } as unknown as DOMRect);

    expect(registry.fitTerminal(sid)).toEqual({ cols: 80, rows: 24 });
    expect(registry.fitTerminal(sid)).toBeNull();
  });
});

describe('スクロールバックの保存/復元（契約 §16「付け替えでスクロールバックが消えない」）', () => {
  it('末尾追従中でなければ detach → attach で同じ行へ scrollToLine する', () => {
    const sid = nextSurfaceId();
    const containerA = document.createElement('div');
    const containerB = document.createElement('div');

    registry.attachTerminal(sid, containerA);
    const term = fakeOf(sid);
    term.buffer = { active: { viewportY: 5, baseY: 10 } };

    registry.detachTerminal(sid);
    registry.attachTerminal(sid, containerB);

    expect(term.scrollToLineCalls).toEqual([5]);
  });

  it('末尾追従中（viewportY >= baseY）なら detach → attach で scrollToBottom する', () => {
    const sid = nextSurfaceId();
    const containerA = document.createElement('div');
    const containerB = document.createElement('div');

    registry.attachTerminal(sid, containerA);
    const term = fakeOf(sid);
    term.buffer = { active: { viewportY: 10, baseY: 10 } };
    const before = term.scrollToBottomCalls;

    registry.detachTerminal(sid);
    registry.attachTerminal(sid, containerB);

    expect(term.scrollToBottomCalls).toBe(before + 1);
    expect(term.scrollToLineCalls).toEqual([]);
  });
});

describe('disposeTerminal', () => {
  it('エントリを削除し listTerminals から消える', () => {
    const sid = nextSurfaceId();
    registry.ensureTerminal(sid);
    expect(registry.listTerminals()).toContain(sid);

    registry.disposeTerminal(sid);
    expect(registry.listTerminals()).not.toContain(sid);
    expect(registry.getTerminal(sid)).toBeUndefined();
  });
});

describe('writeNotice', () => {
  it('ターミナルにメッセージを書き込む', () => {
    const sid = nextSurfaceId();
    registry.writeNotice(sid, 'hello');
    const term = fakeOf(sid);
    expect(term.writes.some((w) => String(w.data).includes('hello'))).toBe(true);
  });
});
