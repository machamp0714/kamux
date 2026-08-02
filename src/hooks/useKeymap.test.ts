import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc/commands', () => ({
  listProjects: vi.fn(),
  createProject: vi.fn(),
  listSessions: vi.fn(),
  createSession: vi.fn(),
  updateSession: vi.fn(),
}));

// React 18 の act() は既定でこのフラグを見て、テスト環境かどうかを判定する
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

import { useAppStore } from '../store';
import { handleKeymapKeyDown, useKeymap } from './useKeymap';

const dispatch = (init: KeyboardEventInit) => {
  const event = new KeyboardEvent('keydown', { ...init, cancelable: true });
  window.dispatchEvent(event);
  return event;
};

describe('handleKeymapKeyDown', () => {
  beforeEach(() => {
    window.addEventListener('keydown', handleKeymapKeyDown);
    useAppStore.setState({ view: 'kanban', modal: null });
  });

  afterEach(() => {
    window.removeEventListener('keydown', handleKeymapKeyDown);
  });

  it('Cmd+N で preventDefault し、create_session モーダルを開く（入力欄に n が入らない）', () => {
    const event = dispatch({ key: 'n', metaKey: true });
    expect(event.defaultPrevented).toBe(true);
    expect(useAppStore.getState().modal).toEqual({ kind: 'create_session' });
  });

  it('Cmd+1 で preventDefault し、カンバン画面へ切り替える', () => {
    useAppStore.setState({ view: 'terminal' });
    const event = dispatch({ key: '1', metaKey: true });
    expect(event.defaultPrevented).toBe(true);
    expect(useAppStore.getState().view).toBe('kanban');
  });

  it('モーダルが開いているときの Escape で preventDefault し、モーダルを閉じる', () => {
    useAppStore.setState({ modal: { kind: 'create_session' } });
    const event = dispatch({ key: 'Escape', metaKey: false });
    expect(event.defaultPrevented).toBe(true);
    expect(useAppStore.getState().modal).toBeNull();
  });

  it('モーダルが開いていないときの Escape は preventDefault しない（dnd-kit のドラッグキャンセルを奪わない）', () => {
    const event = dispatch({ key: 'Escape', metaKey: false });
    expect(event.defaultPrevented).toBe(false);
  });

  it('Cmd なしの n は preventDefault せず、モーダルも開かない', () => {
    const event = dispatch({ key: 'n', metaKey: false });
    expect(event.defaultPrevented).toBe(false);
    expect(useAppStore.getState().modal).toBeNull();
  });

  it('IME 変換中（isComposing）の Escape はモーダルを閉じない（変換キャンセルを奪わない）', () => {
    useAppStore.setState({ modal: { kind: 'create_session' } });
    const event = dispatch({ key: 'Escape', metaKey: false, isComposing: true });
    expect(event.defaultPrevented).toBe(false);
    expect(useAppStore.getState().modal).toEqual({ kind: 'create_session' });
  });

  it('IME 変換中（keyCode 229 フォールバック）の Escape もモーダルを閉じない', () => {
    useAppStore.setState({ modal: { kind: 'create_session' } });
    const event = dispatch({ key: 'Escape', metaKey: false, keyCode: 229 });
    expect(event.defaultPrevented).toBe(false);
    expect(useAppStore.getState().modal).toEqual({ kind: 'create_session' });
  });

  it('IME 変換中でも Cmd+N は効く（契約 §11 の Cmd 系は奪う）', () => {
    const event = dispatch({ key: 'n', metaKey: true, isComposing: true });
    expect(event.defaultPrevented).toBe(true);
    expect(useAppStore.getState().modal).toEqual({ kind: 'create_session' });
  });

  it('Cmd+2 で preventDefault し、ターミナル画面へ切り替える', () => {
    const event = dispatch({ key: '2', metaKey: true });
    expect(event.defaultPrevented).toBe(true);
    expect(useAppStore.getState().view).toBe('terminal');
  });

  it('ターミナル画面での Cmd+J は preventDefault し、cycleSession(1) を呼ぶ（j がシェルに漏れない）', () => {
    useAppStore.setState({ view: 'terminal' });
    const cycleSession = vi.fn();
    useAppStore.setState({ cycleSession });
    const event = dispatch({ key: 'j', metaKey: true });
    expect(event.defaultPrevented).toBe(true);
    expect(cycleSession).toHaveBeenCalledWith(1);
  });

  it('ターミナル画面での Cmd+K は preventDefault し、cycleSession(-1) を呼ぶ（k がシェルに漏れない）', () => {
    useAppStore.setState({ view: 'terminal' });
    const cycleSession = vi.fn();
    useAppStore.setState({ cycleSession });
    const event = dispatch({ key: 'k', metaKey: true });
    expect(event.defaultPrevented).toBe(true);
    expect(cycleSession).toHaveBeenCalledWith(-1);
  });

  it('カンバン画面では Cmd+J/K を無視する（cycleSession を呼ばず、preventDefault もしない）', () => {
    const cycleSession = vi.fn();
    useAppStore.setState({ view: 'kanban', cycleSession });
    const jEvent = dispatch({ key: 'j', metaKey: true });
    const kEvent = dispatch({ key: 'k', metaKey: true });
    expect(cycleSession).not.toHaveBeenCalled();
    expect(jEvent.defaultPrevented).toBe(false);
    expect(kEvent.defaultPrevented).toBe(false);
  });

  it('Cmd なしの j / k は preventDefault せず、cycleSession も呼ばない（シェルの vim キーバインドを奪わない）', () => {
    const cycleSession = vi.fn();
    useAppStore.setState({ view: 'terminal', cycleSession });
    const jEvent = dispatch({ key: 'j', metaKey: false });
    const kEvent = dispatch({ key: 'k', metaKey: false });
    expect(cycleSession).not.toHaveBeenCalled();
    expect(jEvent.defaultPrevented).toBe(false);
    expect(kEvent.defaultPrevented).toBe(false);
  });
});

describe('useKeymap（実フックとして window リスナを 1 本だけ張る。契約 §11.3-1）', () => {
  function Harness() {
    useKeymap();
    return null;
  }

  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    act(() => {
      root.unmount();
    });
    container.remove();
  });

  it('マウント時に window の keydown リスナをちょうど 1 つだけ追加する', () => {
    const addSpy = vi.spyOn(window, 'addEventListener');
    act(() => {
      root = createRoot(container);
      root.render(createElement(Harness));
    });
    const keydownAdds = addSpy.mock.calls.filter(([type]) => type === 'keydown');
    expect(keydownAdds).toHaveLength(1);
    addSpy.mockRestore();
  });

  it('アンマウント時に window の keydown リスナをちょうど 1 つだけ削除する', () => {
    act(() => {
      root = createRoot(container);
      root.render(createElement(Harness));
    });
    const removeSpy = vi.spyOn(window, 'removeEventListener');
    act(() => {
      root.unmount();
    });
    const keydownRemoves = removeSpy.mock.calls.filter(([type]) => type === 'keydown');
    expect(keydownRemoves).toHaveLength(1);
    removeSpy.mockRestore();
  });
});
