import { act } from 'react';
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
    // TerminalPane.test.tsx と同じ理由（vi.fn() は内部で Promise を横取りするため
    // unhandled rejection を検出できない）。回帰テストだけ素の reject へ挿げ替える
    resizePtyImpl: (surfaceId: string, cols: number, rows: number): Promise<void> =>
      resizePty(surfaceId, cols, rows),
    ensurePtySubscription: vi.fn(),
    isStarted: vi.fn(),
    markStarted: vi.fn(),
    unmarkStarted: vi.fn(),
    ensureTerminal: vi.fn(),
    attachTerminal: vi.fn(),
    detachTerminal: vi.fn(),
    fitTerminal: vi.fn(),
    getTerminal: vi.fn(),
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
// disposeTerminal は意図的に生やさない。実装が呼べば「関数ではない」で例外になり、
// 契約 §16（ペイン再割当で dispose を呼ばない）の検出器としてテスト自身が働く
vi.mock('../../terminal/registry', () => ({
  ensureTerminal: mocks.ensureTerminal,
  attachTerminal: mocks.attachTerminal,
  detachTerminal: mocks.detachTerminal,
  fitTerminal: mocks.fitTerminal,
  getTerminal: mocks.getTerminal,
  invalidateFitCache: mocks.invalidateFitCache,
  writeNotice: mocks.writeNotice,
}));

import { useAppStore } from '../../store';
import type { Layout, PaneAssignment, PaneIndex } from '../../store/paneLogic';
import { TerminalGrid } from './TerminalGrid';

declare const process: {
  on(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
  off(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
};

/** ResizeObserver 経路の門を手動発火で検証するため、コールバックを保持する */
class FakeResizeObserver {
  static callbacks: Array<() => void> = [];
  static observed: Element[] = [];

  constructor(cb: () => void) {
    FakeResizeObserver.callbacks.push(cb);
  }

  observe(el: Element): void {
    FakeResizeObserver.observed.push(el);
  }
  disconnect(): void {}
}

async function flush(times = 4): Promise<void> {
  await act(async () => {
    for (let i = 0; i < times; i++) {
      await new Promise<void>((resolve) => queueMicrotask(() => resolve()));
    }
  });
}

let container: HTMLDivElement;
let root: Root;

function setPanes(layout: Layout, paneAssignment: PaneAssignment, activePane: PaneIndex): void {
  useAppStore.setState({
    layout,
    paneAssignment,
    activePane,
    focusedSessionId: paneAssignment[activePane],
  });
}

function render(): void {
  act(() => {
    root = createRoot(container);
    root.render(<TerminalGrid />);
  });
}

function slots(): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>('.terminal-pane-slot'));
}

/** attachTerminal に渡された surface_id を呼び出し順に返す */
function attachedSurfaces(): string[] {
  return mocks.attachTerminal.mock.calls.map((c) => c[0] as string);
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  FakeResizeObserver.callbacks = [];
  FakeResizeObserver.observed = [];
  mocks.isStarted.mockReturnValue(false);
  mocks.ensurePtySubscription.mockReturnValue(new Promise<void>(() => {}));
  mocks.fitTerminal.mockReturnValue(null);
  mocks.getTerminal.mockReturnValue(undefined);
  mocks.resizePty.mockResolvedValue(undefined);
  mocks.resizePtyImpl = (surfaceId: string, cols: number, rows: number) =>
    mocks.resizePty(surfaceId, cols, rows);
  useAppStore.setState({ modal: null, runtimeStates: {}, runtimeReasons: {}, runtimeErrors: {} });
  setPanes('single', ['s1', null], 0);
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

describe('TerminalGrid（DOM 契約 §57.3）', () => {
  it('single では表示中のペインだけを描き、その surface を attach する', async () => {
    render();
    await flush();

    expect(slots()).toHaveLength(1);
    expect(slots()[0].dataset.pane).toBe('0');
    expect(container.querySelector('.terminal-grid')?.getAttribute('data-layout')).toBe('single');
    expect(mocks.attachTerminal).toHaveBeenCalledWith(
      's1:agent',
      container.querySelector('.terminal-pane-slot__host'),
    );
  });

  it('single かつ activePane=1 では裏スロットではなく 1 番のペインを描く', async () => {
    // 軸 B（PR 25 の M8 と同形）。ホスト配列や attach ループが pane 0 前提だと
    // ここだけが赤くなる
    setPanes('single', ['s1', 's2'], 1);
    render();
    await flush();

    expect(slots()).toHaveLength(1);
    expect(slots()[0].dataset.pane).toBe('1');
    expect(attachedSurfaces()).toEqual(['s2:agent']);
    expect(mocks.attachTerminal).toHaveBeenCalledWith(
      's2:agent',
      container.querySelector('.terminal-pane-slot__host'),
    );
  });

  it('split2 では 2 面を左から描き、それぞれ自分のホストへ attach する', async () => {
    setPanes('split2', ['s1', 's2'], 0);
    render();
    await flush();

    const hosts = Array.from(container.querySelectorAll<HTMLElement>('.terminal-pane-slot__host'));
    expect(slots().map((el) => el.dataset.pane)).toEqual(['0', '1']);
    expect(mocks.attachTerminal).toHaveBeenCalledWith('s1:agent', hosts[0]);
    expect(mocks.attachTerminal).toHaveBeenCalledWith('s2:agent', hosts[1]);
  });

  it('split2-v でも 2 面を描き、data-layout だけが変わる（契約 §28.2 / §28.4）', async () => {
    setPanes('split2-v', ['s1', 's2'], 0);
    render();
    await flush();

    expect(container.querySelector('.terminal-grid')?.getAttribute('data-layout')).toBe('split2-v');
    expect(slots().map((el) => el.dataset.pane)).toEqual(['0', '1']);
    expect(attachedSurfaces()).toEqual(['s1:agent', 's2:agent']);
  });

  it('data-active はアクティブペインだけ true（非アクティブ側は false）', async () => {
    setPanes('split2', ['s1', 's2'], 1);
    render();
    await flush();

    expect(slots().map((el) => el.dataset.active)).toEqual(['false', 'true']);
  });

  it('セッション未割当のペインには空表示を出し、attach しない', async () => {
    setPanes('split2', ['s1', null], 0);
    render();
    await flush();

    expect(slots()).toHaveLength(2);
    expect(slots()[1].querySelector('.terminal-pane-slot__empty')).not.toBeNull();
    expect(slots()[0].querySelector('.terminal-pane-slot__empty')).toBeNull();
    expect(attachedSurfaces()).toEqual(['s1:agent']);
  });
});

describe('TerminalGrid（アクティブペインの移動）', () => {
  it('非アクティブ側のスロットを押すと activePane がそのペインへ移る', async () => {
    setPanes('split2', ['s1', 's2'], 0);
    render();
    await flush();

    act(() => {
      slots()[1].dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    });

    expect(useAppStore.getState().activePane).toBe(1);
    expect(slots().map((el) => el.dataset.active)).toEqual(['false', 'true']);
  });

  it('アクティブ側を押しても activePane は動かない', async () => {
    setPanes('split2', ['s1', 's2'], 1);
    render();
    await flush();

    act(() => {
      slots()[1].dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    });

    expect(useAppStore.getState().activePane).toBe(1);
  });
});

describe('TerminalGrid（不変条件 F: 集合差分で attach/detach を駆動する）', () => {
  it('左右スワップでは detachTerminal を 1 度も呼ばない（集合が同じ）', async () => {
    setPanes('split2', ['s1', 's2'], 0);
    render();
    await flush();
    mocks.attachTerminal.mockClear();

    act(() => {
      useAppStore.getState().assignPane(0, 's2');
    });
    await flush();

    expect(useAppStore.getState().paneAssignment).toEqual(['s2', 's1']);
    expect(mocks.detachTerminal).not.toHaveBeenCalled();
    const hosts = Array.from(container.querySelectorAll<HTMLElement>('.terminal-pane-slot__host'));
    expect(mocks.attachTerminal).toHaveBeenCalledWith('s2:agent', hosts[0]);
    expect(mocks.attachTerminal).toHaveBeenCalledWith('s1:agent', hosts[1]);
  });

  it('表示集合から外れたセッションだけを detach する', async () => {
    setPanes('single', ['s1', null], 0);
    render();
    await flush();

    act(() => {
      useAppStore.getState().assignPane(0, 's2');
    });
    await flush();

    expect(mocks.detachTerminal).toHaveBeenCalledWith('s1:agent');
    expect(mocks.detachTerminal).toHaveBeenCalledTimes(1);
    expect(mocks.attachTerminal).toHaveBeenCalledWith(
      's2:agent',
      container.querySelector('.terminal-pane-slot__host'),
    );
  });

  it('split2 → single で見えなくなったペインの surface を detach する', async () => {
    setPanes('split2', ['s1', 's2'], 0);
    render();
    await flush();

    act(() => {
      useAppStore.getState().setLayout('single');
    });
    await flush();

    expect(mocks.detachTerminal).toHaveBeenCalledWith('s2:agent');
    expect(mocks.detachTerminal).not.toHaveBeenCalledWith('s1:agent');
  });

  it('アンマウントで表示中の全 surface を detach する（dispose は呼ばない）', async () => {
    setPanes('split2', ['s1', 's2'], 0);
    render();
    await flush();

    act(() => {
      root.unmount();
    });

    expect(mocks.detachTerminal).toHaveBeenCalledWith('s1:agent');
    expect(mocks.detachTerminal).toHaveBeenCalledWith('s2:agent');
    // 以降の afterEach で二重 unmount しないように貼り直す
    act(() => {
      root = createRoot(container);
      root.render(<TerminalGrid />);
    });
  });
});

describe('TerminalGrid（Important 1: 未起動 PTY への resize_pty を防ぐ門）', () => {
  it('isStarted が false の初回マウントでは fitTerminal を呼ばない', async () => {
    mocks.isStarted.mockReturnValue(false);

    render();
    await flush();

    expect(mocks.fitTerminal).not.toHaveBeenCalled();
  });

  it('isStarted が true の再 attach（画面外にいる間の window resize 相当）では attach 直後に fitTerminal を呼ぶ', async () => {
    mocks.isStarted.mockReturnValue(true);

    render();
    await flush();

    expect(mocks.fitTerminal).toHaveBeenCalledWith('s1:agent');
  });

  it('ResizeObserver 経路にも同じ門がある（isStarted が false の間は resize が来ても fitTerminal を呼ばない）', async () => {
    mocks.isStarted.mockReturnValue(false); // pty://exit 後、まだ再起動していない状態を模す
    mocks.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });

    render();
    await flush();
    expect(mocks.fitTerminal).not.toHaveBeenCalled();

    FakeResizeObserver.callbacks.forEach((cb) => {
      cb();
    });
    await new Promise((resolve) => setTimeout(resolve, 150));
    await flush();

    expect(mocks.fitTerminal).not.toHaveBeenCalled();
  });

  it('ResizeObserver は表示中の各ペインのホストを観測し、門を通れば resize_pty を送る', async () => {
    mocks.isStarted.mockReturnValue(true);
    mocks.fitTerminal.mockReturnValue({ cols: 100, rows: 30 });
    setPanes('split2', ['s1', 's2'], 0);

    render();
    await flush();
    mocks.resizePty.mockClear();

    const hosts = Array.from(container.querySelectorAll<HTMLElement>('.terminal-pane-slot__host'));
    expect(FakeResizeObserver.observed).toEqual(hosts);

    FakeResizeObserver.callbacks.forEach((cb) => {
      cb();
    });
    await new Promise((resolve) => setTimeout(resolve, 150));
    await flush();

    expect(mocks.resizePty).toHaveBeenCalledWith('s1:agent', 100, 30);
    expect(mocks.resizePty).toHaveBeenCalledWith('s2:agent', 100, 30);
  });

  it('isStarted の門を通った resize_pty が reject しても unhandled rejection にならない', async () => {
    mocks.resizePtyImpl = (): Promise<void> =>
      Promise.reject(new Error('NotFound: pty already exited'));
    mocks.isStarted.mockReturnValue(true);
    mocks.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });

    const rejections: unknown[] = [];
    const onUnhandledRejection = (reason: unknown): void => {
      rejections.push(reason);
    };
    process.on('unhandledRejection', onUnhandledRejection);

    try {
      render();
      await flush();
      await new Promise((resolve) => setTimeout(resolve, 0));
      await new Promise((resolve) => setTimeout(resolve, 0));
    } finally {
      process.off('unhandledRejection', onUnhandledRejection);
    }

    expect(rejections).toHaveLength(0);
  });
});

describe('TerminalGrid（契約 §85.5 条件 1: 2 ペインでも二重起動しない）', () => {
  it('左右スワップでセッションが載り替えても start_session はセッションごとに 1 回だけ', async () => {
    // 門（ptyBridge の isStarted / markStarted）は実装ではモジュールスコープの Set。
    // ここでもそれを模して、複数のライフサイクル層の間で状態を共有させる
    const started = new Set<string>();
    mocks.markStarted.mockImplementation((surface: string) => {
      started.add(surface);
    });
    mocks.isStarted.mockImplementation((surface: string) => started.has(surface));
    mocks.ensurePtySubscription.mockResolvedValue(undefined);
    mocks.startSession.mockResolvedValue(undefined);

    setPanes('split2', ['s1', 's2'], 0);
    render();
    await flush();

    expect(mocks.startSession).toHaveBeenCalledTimes(2);

    // スワップ。設計 §3.3 が同一セッションの両ペイン割当を禁じているため、
    // 同じセッションのライフサイクル層が 2 度目にマウントされる経路はこれだけである
    act(() => {
      useAppStore.getState().assignPane(0, 's2');
    });
    await flush();

    expect(useAppStore.getState().paneAssignment).toEqual(['s2', 's1']);
    // 門を消すと s1 / s2 がもう一度起動して 4 回になる
    expect(mocks.startSession).toHaveBeenCalledTimes(2);
    expect(mocks.startSession.mock.calls.map((c) => c[0])).toEqual(['s1', 's2']);
  });
});
