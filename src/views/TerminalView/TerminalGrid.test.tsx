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

/**
 * ResizeObserver 経路の門を手動発火で検証するため、コールバックを保持する。
 *
 * disconnect() は自分の cb を callbacks から外す（Task 8/9 の観測穴: 剪定しない
 * 実装だと、TerminalGrid.tsx が cleanup で observer.disconnect() を呼び忘れても
 * どのテストも赤にならない。実測で 3 本の死んだ observer が積み上がり、
 * resizePty が 6 回出た）。observed は剪定しない —— 剪定すると `:442` / `:468`
 * 付近の「observe された要素」の検証を書き直す必要が出て差分が広がる。
 */
class FakeResizeObserver {
  static callbacks: Array<() => void> = [];
  static observed: Element[] = [];

  private readonly cb: () => void;

  constructor(cb: () => void) {
    this.cb = cb;
    FakeResizeObserver.callbacks.push(cb);
  }

  observe(el: Element): void {
    FakeResizeObserver.observed.push(el);
  }
  disconnect(): void {
    FakeResizeObserver.callbacks = FakeResizeObserver.callbacks.filter((cb) => cb !== this.cb);
  }
}

async function flush(times = 4): Promise<void> {
  await act(async () => {
    for (let i = 0; i < times; i++) {
      await new Promise<void>((resolve) => queueMicrotask(() => resolve()));
    }
  });
}

/**
 * `requestAnimationFrame` / `cancelAnimationFrame` のフェイク（`fitScheduler.test.ts`
 * と同じ形）。壁時計（`setTimeout(resolve, 50)` 等）に依存すると、余裕が失われたときに
 * テストの検出力が静かに失われる（Important 2 の修正ラウンド、契約 §96.2）。
 * `usePaneFit` は `createFitScheduler` を既定の `window.requestAnimationFrame` で
 * 呼ぶため、フック自身にはフェイクを注入する経路が無い。`window` のグローバルを
 * 差し替えることで、`TerminalGrid` 経由のテストでも `fitScheduler.test.ts` と
 * 同じ手動キューを使う。
 */
function fakeRaf() {
  const queue = new Map<number, () => void>();
  let id = 0;
  return {
    raf: (cb: () => void): number => {
      id += 1;
      queue.set(id, cb);
      return id;
    },
    caf: (h: number): void => {
      queue.delete(h);
    },
    tick: (): void => {
      const cbs = [...queue.values()];
      queue.clear();
      cbs.forEach((cb) => cb());
    },
  };
}

let raf: ReturnType<typeof fakeRaf>;

/**
 * フェイク rAF のキューを進めてから、そこで走った副作用（resize_pty 呼び出し等）が
 * 落ち着くまでマイクロタスクを流す。`await new Promise((r) => setTimeout(r, N))` の
 * 置き換え。待ち時間の値に依存しないので、マシンやタイマー解像度が変わっても
 * 検出力が変わらない。
 */
async function rafTick(): Promise<void> {
  await act(async () => {
    raf.tick();
  });
  await flush();
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

/** ペイン index のスロットが持つ attach コンテナ。 */
function hostOf(pane: number): HTMLElement | null {
  return slots()[pane]?.querySelector<HTMLElement>('.terminal-pane-slot__host') ?? null;
}

/**
 * attachTerminal がそのサーフェスに渡した直近のコンテナ。
 *
 * **`toHaveBeenCalledWith(sid, host)` で書いてはならない。** vitest の引数比較は
 * 構造の等価性なので、中身が空で同じクラスの div が 2 つ並ぶ左右ペインでは
 * ホストを取り違えても（`hosts.current[pane]` を `hosts.current[activePane]` に
 * すり替えても）緑のままになる。実測で生き延びた変異である。同一性（toBe）で見ること。
 */
function attachTargetOf(surface: string): unknown {
  const calls = mocks.attachTerminal.mock.calls.filter((c) => c[0] === surface);
  return calls[calls.length - 1]?.[1];
}

/**
 * surface_id をキーに引ける xterm のダミー（TerminalView.test.tsx と同じ形）。
 * getTerminal が undefined を返すままだと「非アクティブなペインにも打鍵の宛先が
 * 移る」変異（isActive を true 固定にする）が観測できない。
 */
const terms = new Map<string, { focus: ReturnType<typeof vi.fn> }>();
function termOf(surface: string): { focus: ReturnType<typeof vi.fn> } {
  const existing = terms.get(surface);
  if (existing !== undefined) return existing;
  const created = { focus: vi.fn() };
  terms.set(surface, created);
  return created;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
  FakeResizeObserver.callbacks = [];
  FakeResizeObserver.observed = [];
  raf = fakeRaf();
  vi.stubGlobal('requestAnimationFrame', raf.raf);
  vi.stubGlobal('cancelAnimationFrame', raf.caf);
  mocks.isStarted.mockReturnValue(false);
  mocks.ensurePtySubscription.mockReturnValue(new Promise<void>(() => {}));
  mocks.fitTerminal.mockReturnValue(null);
  terms.clear();
  mocks.getTerminal.mockImplementation((surface: string) => termOf(surface));
  mocks.resizePty.mockResolvedValue(undefined);
  mocks.resizePtyImpl = (surfaceId: string, cols: number, rows: number) =>
    mocks.resizePty(surfaceId, cols, rows);
  // view: 'terminal' は useActivePaneFocus のガード（契約 §85.4）が要求する。本番では
  // TerminalGrid は view === 'terminal' のときにしか App.tsx からマウントされないが、
  // ここでは TerminalGrid を直接マウントするので明示しておく必要がある
  useAppStore.setState({
    view: 'terminal',
    modal: null,
    runtimeStates: {},
    runtimeReasons: {},
    runtimeErrors: {},
  });
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
    expect(attachTargetOf('s1:agent')).toBe(hostOf(0));
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
    expect(attachTargetOf('s2:agent')).toBe(hostOf(0));
  });

  it('split2 では 2 面を左から描き、それぞれ自分のホストへ attach する', async () => {
    setPanes('split2', ['s1', 's2'], 0);
    render();
    await flush();

    expect(slots().map((el) => el.dataset.pane)).toEqual(['0', '1']);
    // 左のスロットのホストへ左のセッション、右へ右（同一性で見る。上の attachTargetOf 参照）
    expect(attachTargetOf('s1:agent')).toBe(hostOf(0));
    expect(attachTargetOf('s2:agent')).toBe(hostOf(1));
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

  it('ペイン内へ DOM フォーカスが入ると activePane がそちらへ移る（フォーカス層からの復路）', async () => {
    // クリックだけが activePane を動かすのではない。Task 9 の useActivePaneFocus は
    // term.focus() をスロットの中へ当てるので、その復路（onFocusCapture）が無いと
    // DOM フォーカスと activePane が食い違ったまま残る。React は onFocus 系を
    // focusin として受け取る
    setPanes('split2', ['s1', 's2'], 1);
    render();
    await flush();

    act(() => {
      hostOf(0)?.dispatchEvent(new FocusEvent('focusin', { bubbles: true }));
    });

    expect(useAppStore.getState().activePane).toBe(0);
  });

  it('打鍵の宛先はアクティブペインだけ（非アクティブ側のターミナルに focus しない）', async () => {
    // 分割時に「どちらへキー入力が行くか」の実体。isActive を渡し忘れる / true 固定に
    // すると、後から effect が走ったペインが必ずフォーカスを奪う
    setPanes('split2', ['s1', 's2'], 1);
    render();
    await flush();

    expect(termOf('s2:agent').focus).toHaveBeenCalledTimes(1);
    expect(termOf('s1:agent').focus).not.toHaveBeenCalled();
  });

  it('フォーカスは attach の後に当てる（順序が逆だと打鍵の宛先にならない）', async () => {
    // xterm の textarea は attachTerminal でコンテナへ入るまで DOM に無い。
    // attach（親の layout effect）より先に focus（子の passive effect）が走ると
    // フォーカスがどこにも当たらない。この順序は TerminalGrid が attach を
    // useLayoutEffect に置いていることで構造的に保証されている
    setPanes('split2', ['s1', 's2'], 1);
    render();
    await flush();

    const attachOrder = mocks.attachTerminal.mock.invocationCallOrder;
    const focusOrder = termOf('s2:agent').focus.mock.invocationCallOrder;
    expect(attachOrder.length).toBeGreaterThan(0);
    expect(focusOrder.length).toBeGreaterThan(0);
    expect(Math.max(...attachOrder)).toBeLessThan(focusOrder[0]);
  });

  it('activePane=1 から 0 側のスロットを押しても移る（軸 B: 逆向き）', async () => {
    setPanes('split2', ['s1', 's2'], 1);
    render();
    await flush();

    act(() => {
      slots()[0].dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    });

    expect(useAppStore.getState().activePane).toBe(0);
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
    // スワップ後は左右が入れ替わって attach し直される（detach は経ない）
    expect(attachTargetOf('s2:agent')).toBe(hostOf(0));
    expect(attachTargetOf('s1:agent')).toBe(hostOf(1));
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
    expect(attachTargetOf('s2:agent')).toBe(hostOf(0));
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

  it('レイアウトの向きだけが変わっても（split2 → split2-v）可視ペインへ resize_pty が送られる（契約 §28.2）', async () => {
    // paneAssignment / activePane は不変のまま layout だけが split2 → split2-v へ
    // 変わる向き変更。usePaneFit の useEffect deps から layout を外す「最適化」は
    // 契約 §28.2 が名指しで禁止している —— 外すと xterm が横向きの cols/rows を
    // 保持したまま resize_pty に誤ったジオメトリが飛ぶ（表示は崩れないので発覚が
    // 遅れる）。ここではシステム全体としてこの向き変更が re-fit をトリガすることを
    // 固定する
    mocks.isStarted.mockReturnValue(true);
    mocks.fitTerminal.mockReturnValue({ cols: 40, rows: 60 });
    setPanes('split2', ['s1', 's2'], 0);

    render();
    await flush();
    // マウント時点で usePaneFit の状態変化 effect も 1 度発火し、rAF が 1 本
    // ペンディングになる。フェイク rAF（beforeEach で window.requestAnimationFrame /
    // cancelAnimationFrame を差し替え済み）を明示的に進めてから mockClear() する。
    // 壁時計（setTimeout）には依存しない —— 依存すると余裕が失われたときに
    // 検出力が静かに失われる（Important 2 の修正ラウンド、契約 §96.2）
    await rafTick();
    mocks.resizePty.mockClear();

    act(() => {
      useAppStore.getState().setLayout('split2-v');
    });
    await rafTick();

    expect(mocks.resizePty).toHaveBeenCalledWith('s1:agent', 40, 60);
    expect(mocks.resizePty).toHaveBeenCalledWith('s2:agent', 40, 60);
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
    await rafTick();

    expect(mocks.fitTerminal).not.toHaveBeenCalled();
  });

  it('ResizeObserver は表示中の各ペインのホストを観測し、門を通れば resize_pty を送る', async () => {
    mocks.isStarted.mockReturnValue(true);
    mocks.fitTerminal.mockReturnValue({ cols: 100, rows: 30 });
    setPanes('split2', ['s1', 's2'], 0);

    render();
    await flush();
    // マウント時点で usePaneFit の状態変化 effect も 1 度発火し、rAF が 1 本
    // ペンディングになる。フェイク rAF を先にドレインしてから mockClear() しないと、
    // 下の RO コールバック発火が参照する scheduler は「マウント由来のペンディング
    // rAF が既にある」状態を素通りするだけの no-op になり、実際には RO コールバック
    // 本体を空にしてもマウント由来の rAF が stateRef.current を読んで assert を
    // 満たしてしまう（TerminalGrid.tsx の request() 呼び出しが無検証になる事故）
    await rafTick();
    mocks.resizePty.mockClear();

    expect(FakeResizeObserver.observed).toEqual([hostOf(0), hostOf(1)]);

    FakeResizeObserver.callbacks.forEach((cb) => {
      cb();
    });
    await rafTick();

    expect(mocks.resizePty).toHaveBeenCalledWith('s1:agent', 100, 30);
    expect(mocks.resizePty).toHaveBeenCalledWith('s2:agent', 100, 30);

    // (a) 反応配線: レイアウトが動くと effect が張り直され、新しいホストを observe する
    // （M-E: deps を [] に潰すと検出できない）。single へ一旦畳んで split2 に戻すと、
    // pane 1 のスロットは完全に unmount → remount され、ホスト要素は新しい DOM ノードに
    // 差し替わる。effect が張り直されていなければ、この新しいホストは誰にも observe されない
    act(() => {
      useAppStore.getState().setLayout('single');
    });
    await flush();
    act(() => {
      useAppStore.getState().setLayout('split2');
    });
    await flush();

    const rewiredHost1 = hostOf(1);
    expect(rewiredHost1).not.toBeNull();
    expect(FakeResizeObserver.observed).toContain(rewiredHost1);

    // (b) 反応配線: assignPane 後に RO コールバックが発火すると、resizePty の第 1 引数は
    // 【新しい】割当の surface になる（M-F: stateRef.current の更新が消えると、
    // ResizeObserver コールバックは初回マウント時点の古い割当のまま固まる）。
    // mockClear は assignPane の attach effect（useLayoutEffect）の呼び出しを飲み込んだ
    // 【後】、RO コールバックを発火する直前に置く —— 先に置くと attach 経路の
    // resizePty 呼び出しで下の assert が満たされてしまい、RO 経路を見ていることにならない
    act(() => {
      useAppStore.getState().assignPane(0, 's3');
    });
    await flush();
    mocks.resizePty.mockClear();

    FakeResizeObserver.callbacks.forEach((cb) => {
      cb();
    });
    await rafTick();

    expect(mocks.resizePty).toHaveBeenCalledWith('s3:agent', 100, 30);
    expect(mocks.resizePty).not.toHaveBeenCalledWith('s1:agent', 100, 30);
  });

  it('レイアウトを2回変更しても、生きている ResizeObserver は現在の1本だけ（Task 8/9 の観測穴: disconnect の剪定漏れを塞ぐ）', async () => {
    // Task 8 のレビューが実測した形: FakeResizeObserver.disconnect() が空実装だと
    // 死んだ observer が callbacks に積み上がり、レイアウト変更を重ねるたびに
    // resizePty が可視ペイン数の倍数で増えていく（実測: 3 本の死んだ observer ×
    // 2 ペイン = 6 回）。disconnect が正しく自分を剪定していれば、何度レイアウトを
    // 変えても生きている observer は常に 1 本であり、resize_pty は可視ペイン数
    // ぶんしか出ない
    mocks.isStarted.mockReturnValue(true);
    mocks.fitTerminal.mockReturnValue({ cols: 100, rows: 30 });
    setPanes('split2', ['s1', 's2'], 0);

    render();
    await flush();

    act(() => {
      useAppStore.getState().setLayout('single');
    });
    await flush();
    act(() => {
      useAppStore.getState().setLayout('split2');
    });
    await flush();

    // ここまでで RO effect は 3 回張られている（初回マウント + レイアウト変更 2 回）。
    // 生きている observer は最後の 1 本だけのはず
    expect(FakeResizeObserver.callbacks).toHaveLength(1);

    // マウント由来の usePaneFit の rAF をここでドレインしてから mockClear() する
    // （上の RO テストの冒頭コメントと同じ理由）。ドレインしないと、下の RO
    // コールバック発火が実際に resize_pty を送っているかが無検証になる
    await rafTick();
    mocks.resizePty.mockClear();
    FakeResizeObserver.callbacks.forEach((cb) => {
      cb();
    });
    await rafTick();

    // 6 回（3 本の死んだ observer × 2 ペイン）ではなく、可視ペイン数ぶんの 2 回。
    // このアサーション自体は observer リークに対して独立の検出力を持たない
    // （見張りは上の toHaveLength(1)）。共有スケジューラの request() が冪等なので、
    // 死んだ observer が何本あっても flush は 1 回で resize_pty は可視ペイン数ぶんしか
    // 出ない（Task 10 レビュー Minor 3）
    expect(mocks.resizePty).toHaveBeenCalledTimes(2);
  });

  it('usePaneFit の flush（window resize 経由）も isStarted の門を通す（新しい呼び出し口）', async () => {
    // usePaneFit の flush は fitTerminal → resizePty の呼び出し口が TerminalGrid の
    // 中で 3 つ目になる（attach 直後の即時同期 / ResizeObserver / ここ）。
    // paneSize.ts の syncPaneSize は意図的に門を持たないので、呼び出し側の
    // usePaneFit がここで門を通していないと未起動 PTY へ resize_pty が飛ぶ
    mocks.isStarted.mockReturnValue(false);
    mocks.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });

    render();
    await flush();
    mocks.resizePty.mockClear();

    act(() => {
      window.dispatchEvent(new Event('resize'));
    });
    await rafTick();

    expect(mocks.resizePty).not.toHaveBeenCalled();
  });

  it('アンマウント後は保留中の fit 要求も新しい window resize も resize_pty を呼ばない（scheduler.cancel / removeEventListener）', async () => {
    mocks.isStarted.mockReturnValue(true);
    mocks.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });

    render();
    await flush();
    mocks.resizePty.mockClear();

    act(() => {
      window.dispatchEvent(new Event('resize'));
    });
    act(() => {
      root.unmount();
    });
    // scheduler.cancel() が効いていれば、フェイク rAF のキューはこの時点で
    // 空になっている。tick() しても何も呼ばれない（壁時計を待つ必要がない）
    await rafTick();

    expect(mocks.resizePty).not.toHaveBeenCalled();

    // removeEventListener が抜けていると、アンマウント後の resize でも
    // scheduler.request() が呼ばれ続けてしまう
    act(() => {
      window.dispatchEvent(new Event('resize'));
    });
    await rafTick();

    expect(mocks.resizePty).not.toHaveBeenCalled();

    // 以降の afterEach で二重 unmount しないように貼り直す
    act(() => {
      root = createRoot(container);
      root.render(<TerminalGrid />);
    });
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
