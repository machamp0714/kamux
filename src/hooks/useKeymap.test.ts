import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc/commands', () => ({
  listProjects: vi.fn(),
  createProject: vi.fn(),
  listSessions: vi.fn(),
  createSession: vi.fn(),
  updateSession: vi.fn(),
}));

import { useAppStore } from '../store';
import { handleKeymapKeyDown } from './useKeymap';

const dispatch = (init: KeyboardEventInit) => {
  const event = new KeyboardEvent('keydown', { ...init, cancelable: true });
  window.dispatchEvent(event);
  return event;
};

beforeEach(() => {
  window.addEventListener('keydown', handleKeymapKeyDown);
  useAppStore.setState({ view: 'kanban', modal: null });
});

afterEach(() => {
  window.removeEventListener('keydown', handleKeymapKeyDown);
});

describe('handleKeymapKeyDown', () => {
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
});
