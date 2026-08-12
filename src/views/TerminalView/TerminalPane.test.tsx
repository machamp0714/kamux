import { act, StrictMode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// React 18 の act() は既定でこのフラグを見て、テスト環境かどうかを判定する
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// vi.mock はファイル先頭に巻き上げられるので、モック関数は vi.hoisted で先に作る
const mocks = vi.hoisted(() => {
  const resizePty = vi.fn();
  return {
    startSession: vi.fn(),
    resizePty,
    // Important 1（PR 10 fix round 2）の回帰テストは resize_pty の reject が
    // 本当に unhandled rejection にならないかを検証する。vitest の vi.fn() は
    // 呼び出し結果を mock.results に記録するために内部で Promise を横取りして
    // おり、それ自体が Node の「処理済み」判定を成立させてしまう（registry.test.ts
    // の ack_pty のテストと同じ理由）。そのため resizePty（vi.fn）を直接 reject
    // させても unhandled rejection は検出できない。呼び出し口を関数変数として
    // 差し替え可能にしておき、回帰テストだけ素の reject する関数へ挿げ替える。
    resizePtyImpl: (surfaceId: string, cols: number, rows: number): Promise<void> =>
      resizePty(surfaceId, cols, rows),
    ensurePtySubscription: vi.fn(),
    isStarted: vi.fn(),
    markStarted: vi.fn(),
    unmarkStarted: vi.fn(),
    fitTerminal: vi.fn(),
    invalidateFitCache: vi.fn(),
    writeNotice: vi.fn(),
  };
});

vi.mock('../../ipc/commands', () => ({
  startSession: mocks.startSession,
  resizePty: (surfaceId: string, cols: number, rows: number) =>
    mocks.resizePtyImpl(surfaceId, cols, rows),
}));
vi.mock('../../terminal/ptyBridge', () => ({
  ensurePtySubscription: mocks.ensurePtySubscription,
  isStarted: mocks.isStarted,
  markStarted: mocks.markStarted,
  unmarkStarted: mocks.unmarkStarted,
}));
// attachTerminal / detachTerminal / getTerminal は意図的に生やさない。この層は
// DOM を持たず attach/detach を行わず、フォーカスも当てない（契約 §85.5 / §85.6。
// 駆動は TerminalGrid の集合差分、フォーカスは useActivePaneFocus に一本化）ため、
// 実装がどれかを呼べば「関数ではない」で例外になりテストが失敗する
vi.mock('../../terminal/registry', () => ({
  fitTerminal: mocks.fitTerminal,
  invalidateFitCache: mocks.invalidateFitCache,
  writeNotice: mocks.writeNotice,
}));

import { useAppStore } from '../../store';
import type { Session } from '../../types/model';
import { TerminalPane } from './TerminalPane';

/** start_session の戻り値（DB 行のミラー）を組み立てる最小ヘルパ */
function makeSession(overrides: Partial<Session> & { id: string }): Session {
  return {
    project_id: 'p1',
    title: overrides.id,
    description: '',
    kanban_status: 'in_progress',
    sort_order: 1,
    mode: 'worktree',
    branch: null,
    worktree_path: null,
    cli_kind: 'claude',
    cli_command: null,
    claude_session_id: null,
    last_runtime_state: 'idle',
    last_runtime_error: null,
    first_started_at: null,
    heuristics_enabled: true,
    silence_timeout_secs: 30,
    archived_at: null,
    created_at: 0,
    updated_at: 0,
    ...overrides,
  };
}

/**
 * このプロジェクトは `@types/node` を依存に持たないため、`process` はグローバルに
 * 型付けされていない（実行環境が vitest = Node なので実体は存在する）。
 * unhandled rejection の実測にだけ使う（registry.test.ts と同じ形）。
 */
declare const process: {
  on(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
  off(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
};

/** マイクロタスクを複数回消化する。ensurePtySubscription → startSession → 後処理と
 *  Promise チェーンが 3 段あるため、1 回の flush では最後の段まで届かないことがある。
 *  act() で包むことで、消化中に起きる React の状態更新も同期に扱われたことにする */
async function flush(times = 4): Promise<void> {
  await act(async () => {
    for (let i = 0; i < times; i++) {
      await new Promise<void>((resolve) => queueMicrotask(() => resolve()));
    }
  });
}

let container: HTMLDivElement;
let root: Root;

function renderPane(sessionId: string): void {
  act(() => {
    root = createRoot(container);
    root.render(<TerminalPane sessionId={sessionId} />);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.isStarted.mockReturnValue(false);
  mocks.fitTerminal.mockReturnValue(null);
  mocks.resizePty.mockResolvedValue(undefined);
  // 回帰テストが差し替えても、次のテストでは既定の vi.fn() 経由へ必ず戻す
  mocks.resizePtyImpl = (surfaceId: string, cols: number, rows: number) =>
    mocks.resizePty(surfaceId, cols, rows);
  useAppStore.setState({ modal: null, runtimeStates: {}, runtimeReasons: {}, runtimeErrors: {} });
  container = document.createElement('div');
  document.body.appendChild(container);
});

afterEach(() => {
  act(() => {
    root.unmount();
  });
  container.remove();
});

describe('TerminalPane（不変条件 A）', () => {
  it('ensurePtySubscription の解決前は start_session を呼ばない', async () => {
    let resolveSub: () => void = () => {};
    mocks.ensurePtySubscription.mockReturnValue(
      new Promise<void>((resolve) => {
        resolveSub = resolve;
      }),
    );
    mocks.startSession.mockResolvedValue(makeSession({ id: 's1' }));

    renderPane('s1');
    await flush();
    expect(mocks.startSession).not.toHaveBeenCalled();

    resolveSub();
    await flush();
    expect(mocks.startSession).toHaveBeenCalledTimes(1);
    expect(mocks.startSession).toHaveBeenCalledWith('s1');
  });
});

describe('TerminalPane（不変条件 B・D）', () => {
  it('start_session が reject したら unmarkStarted(surface) を呼び、writeNotice に tone="error" で渡す', async () => {
    mocks.ensurePtySubscription.mockResolvedValue(undefined);
    mocks.startSession.mockRejectedValue({ code: 'pty_spawn', message: 'boom' });

    renderPane('s1');
    await flush();

    expect(mocks.unmarkStarted).toHaveBeenCalledWith('s1:agent');
    expect(mocks.writeNotice).toHaveBeenCalledWith(
      's1:agent',
      expect.stringContaining('boom'),
      'error',
    );
  });
});

describe('TerminalPane（契約 §42.3 規約 4: 失敗した起動の痕跡をカードに残す）', () => {
  it('start_session が reject したら setRuntimeError にトーストと同じ文字列を渡す', async () => {
    mocks.ensurePtySubscription.mockResolvedValue(undefined);
    mocks.startSession.mockRejectedValue({
      code: 'pty_spawn',
      message: 'claude: command not found',
    });

    renderPane('s1');
    await flush();

    // mark_error が DB へ書くのと同一の文字列（AppError の Display）をストアにも残す。
    // カードの kanban-card__error はこれを読む
    expect(useAppStore.getState().runtimeErrors['s1']).toBe('claude: command not found');
  });

  it('許可リスト（契約 §40.3）を複製しない —— どの code で落ちても setRuntimeError を呼ぶ', async () => {
    // 二重起動ガードの invalid_state は mark_error の許可リストには**無い**が、
    // フロント側は複製しない（ズレの境界は契約 §42.3.1 が定めている）
    mocks.ensurePtySubscription.mockResolvedValue(undefined);
    mocks.startSession.mockRejectedValue({
      code: 'invalid_state',
      message: 'session s1 is already running',
    });

    renderPane('s1');
    await flush();

    expect(useAppStore.getState().runtimeErrors['s1']).toBe('session s1 is already running');
  });
});

describe('TerminalPane（計画 §4.10: コマンドの戻り値でも seedRuntimeStates を呼ぶ）', () => {
  it('start_session の戻り値の last_runtime_state を seed する（イベント取りこぼしの自己修復）', async () => {
    mocks.ensurePtySubscription.mockResolvedValue(undefined);
    mocks.startSession.mockResolvedValue(
      makeSession({ id: 's1', last_runtime_state: 'running', first_started_at: null }),
    );

    renderPane('s1');
    await flush();

    // 非 reset 経路なので first_started_at === null の除外は適用しない（契約 §34.6）。
    // 戻り値の Session は mark_first_started の非同期コミットより前に読まれて
    // first_started_at === null を持ちうるため、除外すると自己修復が壊れる
    expect(useAppStore.getState().runtimeStates['s1']).toBe('running');
  });
});

describe('TerminalPane（不変条件 C）', () => {
  it('start_session 解決後に invalidateFitCache → fitTerminal の順で再実行する', async () => {
    // isStarted は既定で false（beforeEach）なので、マウント直後の syncSize は
    // Important 1 の門でスキップされる。ここで実行されるのは start_session
    // 解決後の 1 回だけである
    mocks.ensurePtySubscription.mockResolvedValue(undefined);
    mocks.startSession.mockResolvedValue(makeSession({ id: 's1' }));
    const callOrder: string[] = [];
    mocks.invalidateFitCache.mockImplementation(() => {
      callOrder.push('invalidate');
    });
    mocks.fitTerminal.mockImplementation(() => {
      callOrder.push('fit');
      return null;
    });

    renderPane('s1');
    await flush();

    expect(callOrder).toEqual(['invalidate', 'fit']);
    expect(mocks.fitTerminal).toHaveBeenCalledTimes(1);
  });
});

describe('TerminalPane（Important 1: 未起動 PTY への resize_pty を防ぐ門）', () => {
  it('start_session が解決するまで fitTerminal を呼ばない', async () => {
    // この層は attach を持たないので「マウント直後の fit」も持たない
    // （再 attach 経路の門は TerminalGrid 側にある。TerminalGrid.test.tsx 参照）。
    // ここに mount 時 fit を足し戻すと、attach 前に fit する経路が復活する
    mocks.isStarted.mockReturnValue(false);
    mocks.ensurePtySubscription.mockReturnValue(new Promise<void>(() => {}));

    renderPane('s1');
    await flush();

    expect(mocks.fitTerminal).not.toHaveBeenCalled();
  });
});

describe('TerminalPane（Important 1: resize_pty の reject が unhandled rejection にならない・PR 10 fix round 2）', () => {
  it('start_session 解決後の resize_pty が reject しても unhandled rejection にならない', async () => {
    // resizePtyImpl を素の reject する関数へ挿げ替える（vi.fn() 越しでは検出できない。
    // mocks 定義側のコメント参照）
    mocks.resizePtyImpl = (): Promise<void> =>
      Promise.reject(new Error('NotFound: pty already exited'));

    mocks.ensurePtySubscription.mockResolvedValue(undefined);
    mocks.startSession.mockResolvedValue(makeSession({ id: 's1' }));
    mocks.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });

    const rejections: unknown[] = [];
    const onUnhandledRejection = (reason: unknown): void => {
      rejections.push(reason);
    };
    process.on('unhandledRejection', onUnhandledRejection);

    try {
      renderPane('s1');
      await flush();
      // unhandledRejection はイベントループを最低 1 周させないと発火しない
      await new Promise((resolve) => setTimeout(resolve, 0));
      await new Promise((resolve) => setTimeout(resolve, 0));
    } finally {
      process.off('unhandledRejection', onUnhandledRejection);
    }

    expect(mocks.fitTerminal).toHaveBeenCalledWith('s1:agent');
    expect(rejections).toHaveLength(0);
  });
});

// 契約 §85.6: フォーカスは `useActivePaneFocus` に統合され、この層はもう当てない
// （src/views/TerminalView/useActivePaneFocus.test.tsx に移設した）。
// registry モックに getTerminal を生やしていないので、実装がまだ focus() を
// 呼んでいれば「関数ではない」で例外になり、退行の検出器として働く

describe('TerminalPane（ペインの載せ替え）', () => {
  it('sessionId が変わると新しいセッションを起動する（旧セッションは止めない）', async () => {
    mocks.ensurePtySubscription.mockResolvedValue(undefined);
    mocks.startSession.mockResolvedValue(makeSession({ id: 's1' }));

    renderPane('s1');
    await flush();
    expect(mocks.startSession).toHaveBeenCalledWith('s1');

    act(() => {
      root.render(<TerminalPane sessionId="s2" />);
    });
    await flush();

    expect(mocks.ensurePtySubscription).toHaveBeenCalledWith('s2:agent');
    expect(mocks.startSession).toHaveBeenCalledWith('s2');
    // detach / dispose はこの層の責務ではない。registry モックに attachTerminal /
    // detachTerminal / disposeTerminal を生やしていないので、呼べば例外になる
  });
});

describe('TerminalPane（必達 7: StrictMode での冪等性）', () => {
  it('effect が二重実行されても start_session は 1 回だけ', async () => {
    // isStarted / markStarted は実装（ptyBridge.ts）ではモジュールスコープの
    // Set で共有される。ここでもそれを模して、2 回の effect 実行間で状態を共有させる
    let started = false;
    mocks.markStarted.mockImplementation(() => {
      started = true;
    });
    mocks.isStarted.mockImplementation(() => started);

    act(() => {
      root = createRoot(container);
      root.render(
        <StrictMode>
          <TerminalPane sessionId="s1" />
        </StrictMode>,
      );
    });
    await flush();

    // 前提の確認: StrictMode で実際に effect が二重実行されていること。
    // ここが 1 回しかなければ、以下の start_session の assertion は弁別力ゼロなので
    // その場合はテストを失敗させて気づけるようにする
    expect(mocks.ensurePtySubscription.mock.calls.length).toBeGreaterThanOrEqual(2);

    expect(mocks.startSession).toHaveBeenCalledTimes(1);
  });
});
