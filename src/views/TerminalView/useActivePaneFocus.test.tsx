import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// React 18 の act() は既定でこのフラグを見て、テスト環境かどうかを判定する
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// vi.mock はファイル先頭に巻き上げられるので、モック関数は vi.hoisted で先に作る
const mocks = vi.hoisted(() => ({ getTerminal: vi.fn() }));
vi.mock('../../terminal/registry', () => ({ getTerminal: mocks.getTerminal }));

import { useAppStore } from '../../store';
import { useActivePaneFocus } from './useActivePaneFocus';

/** surface_id をキーに引ける xterm のダミー。textarea は既定で null（未合焦）。 */
function fakeTerm(focus = vi.fn()): { textarea: HTMLTextAreaElement | null; focus: () => void } {
  return { textarea: null, focus };
}

/** useActivePaneFocus を呼ぶだけの薄いハーネス。DOM は描かない。 */
function Harness(): null {
  useActivePaneFocus();
  return null;
}

async function flush(times = 2): Promise<void> {
  await act(async () => {
    for (let i = 0; i < times; i++) {
      await new Promise<void>((resolve) => queueMicrotask(() => resolve()));
    }
  });
}

let container: HTMLDivElement;
let root: Root;

function render(): void {
  act(() => {
    root = createRoot(container);
    root.render(<Harness />);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  useAppStore.setState({
    view: 'terminal',
    modal: null,
    layout: 'single',
    paneAssignment: ['s1', null],
    activePane: 0,
  });
  container = document.createElement('div');
  document.body.appendChild(container);
});

afterEach(() => {
  act(() => {
    root.unmount();
  });
  container.remove();
});

describe('useActivePaneFocus（契約 §85.4: modal のガード）', () => {
  it('modal が null なら activePane の端末に focus する', async () => {
    const focus = vi.fn();
    mocks.getTerminal.mockReturnValue(fakeTerm(focus));

    render();
    await flush();

    expect(mocks.getTerminal).toHaveBeenCalledWith('s1:agent');
    expect(focus).toHaveBeenCalledTimes(1);
  });

  it('modal が開いていれば focus しない（実 $SHELL の PTY へ打鍵が流れるのを防ぐ）', async () => {
    useAppStore.setState({ modal: { kind: 'create_session' } });
    const focus = vi.fn();
    mocks.getTerminal.mockReturnValue(fakeTerm(focus));

    render();
    await flush();

    expect(focus).not.toHaveBeenCalled();
  });
});

describe('useActivePaneFocus（契約 §85.4: modal の依存配列）', () => {
  it('モーダルを閉じると（再マウントなしで）フォーカスが戻る', async () => {
    useAppStore.setState({ modal: { kind: 'create_session' } });
    const focus = vi.fn();
    mocks.getTerminal.mockReturnValue(fakeTerm(focus));

    render();
    await flush();
    expect(focus).not.toHaveBeenCalled();

    act(() => {
      useAppStore.setState({ modal: null });
    });
    await flush();

    expect(focus).toHaveBeenCalledTimes(1);
  });
});

describe('useActivePaneFocus（activePane の依存配列）', () => {
  it('activePane が動くと新しいペインの端末に focus する（再マウントなしで）', async () => {
    useAppStore.setState({ layout: 'split2', paneAssignment: ['s1', 's2'], activePane: 0 });
    const focus1 = vi.fn();
    const focus2 = vi.fn();
    mocks.getTerminal.mockImplementation((surface: string) =>
      surface === 's1:agent' ? fakeTerm(focus1) : fakeTerm(focus2),
    );

    render();
    await flush();
    expect(focus1).toHaveBeenCalledTimes(1);
    expect(focus2).not.toHaveBeenCalled();

    act(() => {
      useAppStore.setState({ activePane: 1 });
    });
    await flush();

    expect(focus2).toHaveBeenCalledTimes(1);
  });
});

describe('useActivePaneFocus（§85.5 条件 2: focus:// / カードクリックの着地点を失わない）', () => {
  it('focusSession が activePane を動かすと、新しく割り当てられたペインの端末に focus が当たる', async () => {
    // split2 で s2 が既に右ペインに載っている状態から focusSession('s2') を呼ぶと、
    // routeFocusReducer は paneAssignment を動かさず activePane だけ 1 へ移す
    // （contract §85.5.1 が実物で確認した経路そのもの）
    useAppStore.setState({ layout: 'split2', paneAssignment: ['s1', 's2'], activePane: 0 });
    const focus1 = vi.fn();
    const focus2 = vi.fn();
    mocks.getTerminal.mockImplementation((surface: string) =>
      surface === 's1:agent' ? fakeTerm(focus1) : fakeTerm(focus2),
    );

    render();
    await flush();
    expect(focus2).not.toHaveBeenCalled();

    act(() => {
      useAppStore.getState().focusSession('s2', 'terminal');
    });
    await flush();

    expect(useAppStore.getState().activePane).toBe(1);
    expect(useAppStore.getState().paneAssignment).toEqual(['s1', 's2']);
    expect(focus2).toHaveBeenCalledTimes(1);
  });

  it('モーダル表示中に focusSession しても focus しない', async () => {
    useAppStore.setState({
      layout: 'split2',
      paneAssignment: ['s1', 's2'],
      activePane: 0,
      modal: { kind: 'create_session' },
    });
    const focus2 = vi.fn();
    mocks.getTerminal.mockImplementation((surface: string) =>
      surface === 's2:agent' ? fakeTerm(focus2) : fakeTerm(),
    );

    render();
    await flush();

    act(() => {
      useAppStore.getState().focusSession('s2', 'terminal');
    });
    await flush();

    // 着地（activePane の移動）そのものは modal と無関係に起きるが、
    // 実シェルへの DOM フォーカスはモーダルが閉じるまで渡らない
    expect(useAppStore.getState().activePane).toBe(1);
    expect(focus2).not.toHaveBeenCalled();
  });
});

describe('useActivePaneFocus（自作変異 1: view のガード）', () => {
  it('view が terminal でなければ focus しない', async () => {
    useAppStore.setState({ view: 'kanban' });
    const focus = vi.fn();
    mocks.getTerminal.mockReturnValue(fakeTerm(focus));

    render();
    await flush();

    expect(focus).not.toHaveBeenCalled();
  });
});

describe('useActivePaneFocus（自作変異 2: 冪等ガード）', () => {
  it('既に合焦している textarea には再度 focus() しない', async () => {
    const textarea = document.createElement('textarea');
    document.body.appendChild(textarea);
    textarea.focus();
    const focus = vi.fn();
    mocks.getTerminal.mockReturnValue({ textarea, focus });

    render();
    await flush();

    expect(document.activeElement).toBe(textarea);
    expect(focus).not.toHaveBeenCalled();

    textarea.remove();
  });
});
