import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc/commands', () => ({
  listProjects: vi.fn(),
  createProject: vi.fn(),
  listSessions: vi.fn(),
  createSession: vi.fn(),
  updateSession: vi.fn(),
}));

import { useAppStore } from './index';

// createUiSlice が実際に生成する初期値をプリミティブとして捕獲する。
// beforeEach で 'kanban' を書き戻すと「初期ビューはカンバン」テストが
// beforeEach 自身を検証してしまい、実装の初期値を壊しても赤くならない
// （自己成就アサート）。ここでは実装の初期値そのものを検証対象にする。
const { view: initialView, focusedSessionId: initialFocusedSessionId } = useAppStore.getState();

beforeEach(() => {
  useAppStore.setState({ view: initialView, focusedSessionId: initialFocusedSessionId });
});

describe('uiSlice', () => {
  it('初期ビューはカンバン', () => {
    expect(initialView).toBe('kanban');
    expect(initialFocusedSessionId).toBeNull();
  });

  it('setView がビューを切り替える', () => {
    useAppStore.getState().setView('terminal');
    expect(useAppStore.getState().view).toBe('terminal');
    useAppStore.getState().setView('editor');
    expect(useAppStore.getState().view).toBe('editor');
  });

  it('focusSession は既定でビューを変えずにフォーカスだけ動かす', () => {
    useAppStore.getState().setView('terminal');
    useAppStore.getState().focusSession('s1');
    expect(useAppStore.getState().focusedSessionId).toBe('s1');
    expect(useAppStore.getState().view).toBe('terminal');
  });

  it('focusSession に view を渡すと同時に切り替える（カードクリックの経路）', () => {
    useAppStore.getState().focusSession('s1', 'terminal');
    expect(useAppStore.getState().focusedSessionId).toBe('s1');
    expect(useAppStore.getState().view).toBe('terminal');
  });
});
