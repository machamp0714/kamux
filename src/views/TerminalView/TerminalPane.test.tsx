import { act, StrictMode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// React 18 の act() は既定でこのフラグを見て、テスト環境かどうかを判定する
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// vi.mock はファイル先頭に巻き上げられるので、モック関数は vi.hoisted で先に作る
const mocks = vi.hoisted(() => ({
  startSession: vi.fn(),
  resizePty: vi.fn(),
  ensurePtySubscription: vi.fn(),
  isStarted: vi.fn(),
  markStarted: vi.fn(),
  unmarkStarted: vi.fn(),
  attachTerminal: vi.fn(),
  detachTerminal: vi.fn(),
  fitTerminal: vi.fn(),
  invalidateFitCache: vi.fn(),
  writeNotice: vi.fn(),
}));

vi.mock('../../ipc/commands', () => ({
  startSession: mocks.startSession,
  resizePty: mocks.resizePty,
}));
vi.mock('../../terminal/ptyBridge', () => ({
  ensurePtySubscription: mocks.ensurePtySubscription,
  isStarted: mocks.isStarted,
  markStarted: mocks.markStarted,
  unmarkStarted: mocks.unmarkStarted,
}));
vi.mock('../../terminal/registry', () => ({
  attachTerminal: mocks.attachTerminal,
  detachTerminal: mocks.detachTerminal,
  fitTerminal: mocks.fitTerminal,
  invalidateFitCache: mocks.invalidateFitCache,
  writeNotice: mocks.writeNotice,
}));

import { TerminalPane } from './TerminalPane';

class FakeResizeObserver {
  observe(): void {}
  disconnect(): void {}
}

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

function renderPane(sessionId: string | null): void {
  act(() => {
    root = createRoot(container);
    root.render(<TerminalPane sessionId={sessionId} />);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  mocks.isStarted.mockReturnValue(false);
  mocks.fitTerminal.mockReturnValue(null);
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

describe('TerminalPane（不変条件 A）', () => {
  it('ensurePtySubscription の解決前は start_session を呼ばない', async () => {
    let resolveSub: () => void = () => {};
    mocks.ensurePtySubscription.mockReturnValue(
      new Promise<void>((resolve) => {
        resolveSub = resolve;
      }),
    );
    mocks.startSession.mockResolvedValue(undefined);

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

describe('TerminalPane（不変条件 C）', () => {
  it('start_session 解決後に invalidateFitCache → fitTerminal の順で再実行する', async () => {
    mocks.ensurePtySubscription.mockResolvedValue(undefined);
    mocks.startSession.mockResolvedValue(undefined);
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

    // マウント直後の syncSize（1 回目の fit）→ start_session 解決後の invalidate → 2 回目の fit
    expect(callOrder).toEqual(['fit', 'invalidate', 'fit']);
    expect(mocks.fitTerminal).toHaveBeenCalledTimes(2);
  });
});

describe('TerminalPane（不変条件 F）', () => {
  it('ペイン再割当で detachTerminal(旧 surface) が呼ばれ、disposeTerminal は呼ばれない', async () => {
    mocks.ensurePtySubscription.mockResolvedValue(undefined);
    mocks.startSession.mockResolvedValue(undefined);

    renderPane('s1');
    await flush();
    expect(mocks.attachTerminal).toHaveBeenCalledWith('s1:agent', expect.any(HTMLElement));

    act(() => {
      root.render(<TerminalPane sessionId="s2" />);
    });
    await flush();

    expect(mocks.detachTerminal).toHaveBeenCalledWith('s1:agent');
    expect(mocks.attachTerminal).toHaveBeenCalledWith('s2:agent', expect.any(HTMLElement));
    // registry モックには disposeTerminal を生やしていない。実装が呼べば
    // 「関数ではない」で例外になりテストが失敗する（このテスト自体が不変条件の検出器）
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
    expect(mocks.attachTerminal.mock.calls.length).toBeGreaterThanOrEqual(2);

    expect(mocks.startSession).toHaveBeenCalledTimes(1);
  });
});
