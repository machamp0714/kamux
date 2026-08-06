import { act, StrictMode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// React 18 の act() は既定でこのフラグを見て、テスト環境かどうかを判定する
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// vi.mock はファイル先頭に巻き上げられるので、モック関数は vi.hoisted で先に作る
const mocks = vi.hoisted(() => {
  const resizePty = vi.fn();
  return {
    spawnEditor: vi.fn(),
    resizePty,
    // resize_pty の reject が unhandled rejection にならないことの検証は、vi.fn() 越し
    // では実測できない（vitest が mock.results 用に Promise を横取りし、それ自体が
    // 「処理済み」判定を成立させる。TerminalPane.test.tsx と同じ理由）。呼び出し口を
    // 関数変数にして、その回帰テストだけ素の reject する関数へ挿げ替える
    resizePtyImpl: (surfaceId: string, cols: number, rows: number): Promise<void> =>
      resizePty(surfaceId, cols, rows),
    onPtyExit: vi.fn(),
    ensurePtySubscription: vi.fn(),
    ensureTerminal: vi.fn(),
    attachTerminal: vi.fn(),
    detachTerminal: vi.fn(),
    fitTerminal: vi.fn(),
    getTerminal: vi.fn(),
  };
});

vi.mock('../../ipc/commands', () => ({
  spawnEditor: mocks.spawnEditor,
  resizePty: (surfaceId: string, cols: number, rows: number) =>
    mocks.resizePtyImpl(surfaceId, cols, rows),
}));
vi.mock('../../ipc/events', () => ({ onPtyExit: mocks.onPtyExit }));
vi.mock('../../terminal/ptyBridge', () => ({
  ensurePtySubscription: mocks.ensurePtySubscription,
}));
// registry のモックに disposeTerminal / disposePtySubscription を**生やさない**。
// EditorSurface が呼べば「関数ではない」で例外になり、このファイル自体が
// 契約 §16（ペイン切替では dispose しない）の検出器になる（TerminalPane.test.tsx と同じ形）
vi.mock('../../terminal/registry', () => ({
  ensureTerminal: mocks.ensureTerminal,
  attachTerminal: mocks.attachTerminal,
  detachTerminal: mocks.detachTerminal,
  fitTerminal: mocks.fitTerminal,
  getTerminal: mocks.getTerminal,
}));

import { useAppStore } from '../../store';
import type { PtyExitPayload } from '../../ipc/events';
import { EditorSurface } from './EditorSurface';

/**
 * このプロジェクトは `@types/node` を依存に持たないため、`process` はグローバルに
 * 型付けされていない（実行環境が vitest = Node なので実体は存在する）。
 * unhandled rejection の実測にだけ使う（TerminalPane.test.tsx と同じ形）。
 */
declare const process: {
  on(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
  off(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
};

/** マイクロタスクを複数回消化する（onPtyExit → ensurePtySubscription → spawnEditor の 3 段） */
async function flush(times = 6): Promise<void> {
  await act(async () => {
    for (let i = 0; i < times; i++) {
      await new Promise<void>((resolve) => queueMicrotask(() => resolve()));
    }
  });
}

class FakeResizeObserver {
  static callbacks: Array<() => void> = [];

  constructor(cb: () => void) {
    FakeResizeObserver.callbacks.push(cb);
  }

  observe(): void {}
  disconnect(): void {}
}

let container: HTMLDivElement;
let root: Root;

function renderSurface(sessionId: string): void {
  act(() => {
    root = createRoot(container);
    root.render(<EditorSurface sessionId={sessionId} />);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  FakeResizeObserver.callbacks = [];
  // jsdom の offsetWidth / offsetHeight は常に 0 なので、実寸ガード（設計判断 D7）を
  // 越えられない。fit / resize の経路を踏むテストのために実寸を持たせる
  Object.defineProperty(HTMLElement.prototype, 'offsetWidth', { configurable: true, value: 800 });
  Object.defineProperty(HTMLElement.prototype, 'offsetHeight', { configurable: true, value: 600 });
  mocks.ensureTerminal.mockImplementation(() => ({ options: {} }));
  mocks.onPtyExit.mockResolvedValue(() => {});
  mocks.ensurePtySubscription.mockResolvedValue(undefined);
  mocks.spawnEditor.mockResolvedValue('s1:editor');
  mocks.fitTerminal.mockReturnValue(null);
  mocks.getTerminal.mockReturnValue(undefined);
  mocks.resizePty.mockResolvedValue(undefined);
  mocks.resizePtyImpl = (surfaceId: string, cols: number, rows: number) =>
    mocks.resizePty(surfaceId, cols, rows);
  useAppStore.setState({ modal: null, editorSurfaces: {} });
  container = document.createElement('div');
  document.body.appendChild(container);
});

afterEach(() => {
  act(() => {
    root.unmount();
  });
  container.remove();
  vi.unstubAllGlobals();
});

describe('EditorSurface（契約 §16: 購読の解決を待ってから spawn する）', () => {
  it('ensurePtySubscription の解決前は spawn_editor を呼ばない', async () => {
    let resolveSub: () => void = () => {};
    mocks.ensurePtySubscription.mockReturnValue(
      new Promise<void>((resolve) => {
        resolveSub = resolve;
      }),
    );

    renderSurface('s1');
    await flush();
    expect(mocks.spawnEditor).not.toHaveBeenCalled();

    resolveSub();
    await flush();
    expect(mocks.spawnEditor).toHaveBeenCalledTimes(1);
    expect(mocks.spawnEditor).toHaveBeenCalledWith('s1');
  });

  it('attachTerminal も購読の解決後（surface_id は :editor）', async () => {
    mocks.ensurePtySubscription.mockReturnValue(new Promise<void>(() => {}));

    renderSurface('s1');
    await flush();

    expect(mocks.attachTerminal).not.toHaveBeenCalled();
  });

  it('購読が解決したら attachTerminal(sid, container) を呼ぶ', async () => {
    renderSurface('s1');
    await flush();

    expect(mocks.attachTerminal).toHaveBeenCalledWith('s1:editor', expect.any(HTMLElement));
  });
});

describe('EditorSurface（StrictMode での冪等性）', () => {
  it('effect が二重実行されても spawn_editor は 1 回だけ', async () => {
    act(() => {
      root = createRoot(container);
      root.render(
        <StrictMode>
          <EditorSurface sessionId="s1" />
        </StrictMode>,
      );
    });
    await flush();

    // 前提の確認: StrictMode で実際に effect が二重実行され、cleanup も走っていること。
    // ここが 1 回しか無ければ以下の assertion は弁別力ゼロなので、その場合も失敗させる
    expect(mocks.ensureTerminal.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(mocks.detachTerminal).toHaveBeenCalled();

    // 「起動済みガード」を最初の await より前に置くと、2 回目の setup が 1 回目の書いた
    // spawning を見て自分を止めてしまい、spawn_editor が **1 度も飛ばない**
    expect(mocks.spawnEditor).toHaveBeenCalledTimes(1);
    expect(useAppStore.getState().editorSurfaces['s1']).toEqual({ kind: 'live' });
  });
});

describe('EditorSurface（遅延起動は 1 セッション 1 回）', () => {
  it('既に状態が登録済み（live）なら spawn_editor を呼ばない', async () => {
    useAppStore.setState({ editorSurfaces: { s1: { kind: 'live' } } });

    renderSurface('s1');
    await flush();

    expect(mocks.spawnEditor).not.toHaveBeenCalled();
    // 表示のための配線（attach / 購読）は再マウントでも行う
    expect(mocks.attachTerminal).toHaveBeenCalledWith('s1:editor', expect.any(HTMLElement));
  });

  it('spawn_editor が reject したら error を message ごとストアへ残す', async () => {
    mocks.spawnEditor.mockRejectedValue({
      code: 'cli_not_found',
      message: '`nvim` が見つかりませんでした。',
    });

    renderSurface('s1');
    await flush();

    expect(useAppStore.getState().editorSurfaces['s1']).toEqual({
      kind: 'error',
      message: '`nvim` が見つかりませんでした。',
    });
  });
});

describe('EditorSurface（Task 5 の xterm 設定を適用する）', () => {
  it('ensureTerminal で得た term に macOptionIsMeta を立てる（契約 §19）', async () => {
    const term = { options: {} as { macOptionIsMeta?: boolean } };
    mocks.ensureTerminal.mockReturnValue(term);

    renderSurface('s1');
    await flush();

    expect(term.options.macOptionIsMeta).toBe(true);
  });
});

describe('EditorSurface（再起動 UI 用の pty://exit 購読）', () => {
  it('pty://exit を受けたら exited を exit code ごとストアへ書く', async () => {
    let handler: ((p: PtyExitPayload) => void) | null = null;
    mocks.onPtyExit.mockImplementation((_sid: string, h: (p: PtyExitPayload) => void) => {
      handler = h;
      return Promise.resolve(() => {});
    });

    renderSurface('s1');
    await flush();

    expect(mocks.onPtyExit).toHaveBeenCalledWith('s1:editor', expect.any(Function));
    act(() => {
      handler?.({ surface_id: 's1:editor', exit_code: 0 });
    });

    expect(useAppStore.getState().editorSurfaces['s1']).toEqual({ kind: 'exited', exitCode: 0 });
  });

  it('アンマウントで exit 購読を解除し、detachTerminal だけを呼ぶ（dispose は呼ばない）', async () => {
    const unlisten = vi.fn();
    mocks.onPtyExit.mockResolvedValue(unlisten);

    renderSurface('s1');
    await flush();

    act(() => {
      root.render(<div />);
    });

    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(mocks.detachTerminal).toHaveBeenCalledWith('s1:editor');
    // registry / ptyBridge のモックには disposeTerminal / disposePtySubscription を
    // 生やしていない。実装が呼べば「関数ではない」で例外になりこのテストが落ちる
  });
});

describe('EditorSurface（契約 §16 / §11.4.6: modal === null のときにだけフォーカスを当てる）', () => {
  it('modal が null なら getTerminal(sid).focus() を呼ぶ', async () => {
    const focus = vi.fn();
    mocks.getTerminal.mockReturnValue({ focus });
    useAppStore.setState({ modal: null });

    renderSurface('s1');
    await flush();

    expect(mocks.getTerminal).toHaveBeenCalledWith('s1:editor');
    expect(focus).toHaveBeenCalledTimes(1);
  });

  /**
   * TerminalPane は attachTerminal を effect の同期部分で呼ぶので、後続の
   * フォーカス effect が走る時点で host は既に開いている。EditorSurface の
   * attachTerminal は購読の解決を待つ（契約 §16）ぶんだけ後ろにずれるため、
   * マウント直後に focus() しても display:none の host に当たって何も起きない
   * ——「effect は走ったのにフォーカスは無い」という一番気づきにくい形になる。
   */
  it('attachTerminal が済むまでフォーカスを当てない（済んだ直後に当てる）', async () => {
    const focus = vi.fn();
    mocks.getTerminal.mockReturnValue({ focus });
    let resolveSub: () => void = () => {};
    mocks.ensurePtySubscription.mockReturnValue(
      new Promise<void>((resolve) => {
        resolveSub = resolve;
      }),
    );

    renderSurface('s1');
    await flush();
    expect(mocks.attachTerminal).not.toHaveBeenCalled();
    expect(focus).not.toHaveBeenCalled();

    resolveSub();
    await flush();

    expect(mocks.attachTerminal).toHaveBeenCalledWith('s1:editor', expect.any(HTMLElement));
    expect(focus).toHaveBeenCalledTimes(1);
  });

  it('modal が開いていれば focus しない（打鍵が nvim へ流れるのを防ぐ）', async () => {
    const focus = vi.fn();
    mocks.getTerminal.mockReturnValue({ focus });
    useAppStore.setState({ modal: { kind: 'create_session' } });

    renderSurface('s1');
    await flush();

    expect(focus).not.toHaveBeenCalled();
  });

  it('モーダルを閉じると（再マウントなしで）フォーカスが戻る', async () => {
    const focus = vi.fn();
    mocks.getTerminal.mockReturnValue({ focus });
    useAppStore.setState({ modal: { kind: 'create_session' } });

    renderSurface('s1');
    await flush();
    expect(focus).not.toHaveBeenCalled();

    // コンポーネントを作り直すのではなく、購読しているストアの modal だけを更新する
    // ——「マウント時に 1 度評価する」ではなく modal の遷移に追従することの検証
    act(() => {
      useAppStore.setState({ modal: null });
    });
    await flush();

    expect(focus).toHaveBeenCalledTimes(1);
  });
});

/**
 * `resize_pty` は spawn_editor より前（購読解決の直後）に飛ぶので、PTY がまだ存在せず
 * NotFound で reject する。registry.ts の ackPty / writePty、TerminalPane の syncSize と
 * 同じ理由で握り潰さないと、エディタを開くたびに unhandled promise rejection が出る。
 */
describe('EditorSurface（resize_pty の reject が unhandled rejection にならない）', () => {
  it('まだ存在しない PTY への resize_pty が reject しても unhandled rejection にならない', async () => {
    mocks.resizePtyImpl = (): Promise<void> =>
      Promise.reject(new Error('NotFound: pty does not exist yet'));
    mocks.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });

    const rejections: unknown[] = [];
    const onUnhandledRejection = (reason: unknown): void => {
      rejections.push(reason);
    };
    process.on('unhandledRejection', onUnhandledRejection);

    try {
      renderSurface('s1');
      await flush();
      await new Promise((resolve) => setTimeout(resolve, 0));
      await new Promise((resolve) => setTimeout(resolve, 0));
    } finally {
      process.off('unhandledRejection', onUnhandledRejection);
    }

    expect(rejections).toHaveLength(0);
  });
});
