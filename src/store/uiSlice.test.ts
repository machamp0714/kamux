import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc/commands', () => ({
  listProjects: vi.fn(),
  createProject: vi.fn(),
  listSessions: vi.fn(),
  createSession: vi.fn(),
  updateSession: vi.fn(),
}));

import { useAppStore } from './index';
import { toAppError } from './uiSlice';

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

  // 始点を 'terminal' と 'editor' の 2 通り検証する。1 つの始点だけだと
  // 「view を特定の固定値にすり替える」変異体（例: view ?? 'terminal'）を
  // その固定値と始点が一致した場合に見逃す。2 通りあれば、変異体が書ける
  // 固定値は 1 つしかないため、必ずどちらか一方で赤くなる。
  it.each(['terminal', 'editor'] as const)(
    'focusSession は既定でビューを変えずにフォーカスだけ動かす（始点: %s）',
    (startView) => {
      useAppStore.getState().setView(startView);
      useAppStore.getState().focusSession('s1');
      expect(useAppStore.getState().focusedSessionId).toBe('s1');
      expect(useAppStore.getState().view).toBe(startView);
    },
  );

  // target を 'terminal' と 'editor' の 2 通り検証する。1 つの target だけだと
  // 「渡された view を無視して特定の固定値にすり替える」変異体（例: view を無視して
  // 常に 'terminal' をセットする）を、その固定値と target が一致した場合に見逃す。
  // 2 通りあれば、変異体が書ける固定値は 1 つしかないため、必ずどちらか一方で赤くなる。
  it.each(['terminal', 'editor'] as const)(
    'focusSession に view を渡すと同時に切り替える（カードクリックの経路、target: %s）',
    (targetView) => {
      useAppStore.getState().focusSession('s1', targetView);
      expect(useAppStore.getState().focusedSessionId).toBe('s1');
      expect(useAppStore.getState().view).toBe(targetView);
    },
  );
});

describe('uiSlice のモーダル', () => {
  beforeEach(() => {
    useAppStore.setState({ modal: null, lastError: null, view: 'terminal' });
  });

  it('初期状態ではモーダルは開いていない', () => {
    expect(useAppStore.getState().modal).toBeNull();
  });

  it('openModal はモーダルを開くと同時にカンバン画面へ切り替える', () => {
    useAppStore.getState().openModal({ kind: 'create_session' });
    expect(useAppStore.getState().modal).toEqual({ kind: 'create_session' });
    expect(useAppStore.getState().view).toBe('kanban');
  });

  it('openModal は編集対象の sessionId を保持する', () => {
    useAppStore.getState().openModal({ kind: 'edit_session', sessionId: 's1' });
    expect(useAppStore.getState().modal).toEqual({ kind: 'edit_session', sessionId: 's1' });
  });

  it('closeModal でモーダルが閉じる', () => {
    useAppStore.getState().openModal({ kind: 'create_session' });
    useAppStore.getState().closeModal();
    expect(useAppStore.getState().modal).toBeNull();
  });

  it('closeModal は view を戻さない（カンバンに留まる）', () => {
    useAppStore.getState().openModal({ kind: 'create_session' });
    useAppStore.getState().closeModal();
    expect(useAppStore.getState().view).toBe('kanban');
  });
});

describe('uiSlice のエラー', () => {
  beforeEach(() => {
    useAppStore.setState({ lastError: null });
  });

  it('setError でエラーを保持し、null で消せる', () => {
    useAppStore.getState().setError({ code: 'db', message: 'disk full' });
    expect(useAppStore.getState().lastError).toEqual({ code: 'db', message: 'disk full' });
    useAppStore.getState().setError(null);
    expect(useAppStore.getState().lastError).toBeNull();
  });
});

describe('toAppError', () => {
  it('Rust から来た AppError 形状はそのまま通す', () => {
    expect(toAppError({ code: 'git', message: 'fatal: bad revision' })).toEqual({
      code: 'git',
      message: 'fatal: bad revision',
    });
  });

  it('AppError でない値は io として包む', () => {
    expect(toAppError(new Error('boom'))).toEqual({ code: 'io', message: 'Error: boom' });
    expect(toAppError('plain string')).toEqual({ code: 'io', message: 'plain string' });
    expect(toAppError(null)).toEqual({ code: 'io', message: 'null' });
  });
});
