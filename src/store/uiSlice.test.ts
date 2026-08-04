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

  // 始点を 'kanban' と 'editor' の 2 通り検証する。1 つの始点だけだと
  // 「view を無視して固定値にすり替える」変異体を、その固定値と始点が一致した
  // 場合に見逃す。2 通りあれば、変異体が書ける固定値は 1 つしかないため、
  // 必ずどちらか一方で赤くなる。
  //
  // 契約 §11「Enter / クリック（カード）→ focusSession(id, 'terminal')」の
  // 既定値を固定する（M1-4 Task 11 で view の既定を 'terminal' にした。
  // M1-2 PR6 時点のスタブは「既定では view を変えない」だったが、本番の
  // 呼び出し元は必ず view を明示するため、その挙動を検証していたテストは無かった）。
  it.each(['kanban', 'editor'] as const)(
    'focusSession は view を省略すると terminal を既定にする（始点: %s）',
    (startView) => {
      useAppStore.getState().setView(startView);
      useAppStore.getState().focusSession('s1');
      expect(useAppStore.getState().focusedSessionId).toBe('s1');
      expect(useAppStore.getState().view).toBe('terminal');
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

describe('focusSession（要件5: カードクリックで該当ペインにフォーカス）', () => {
  beforeEach(() => {
    useAppStore.setState({
      view: 'kanban',
      focusedSessionId: null,
      layout: 'single',
      activePane: 0,
      paneAssignment: [null, null],
    });
  });

  it('ターミナル画面へ切り替え、アクティブペインにセッションを割り当てる', () => {
    useAppStore.getState().focusSession('sess-1', 'terminal');

    const s = useAppStore.getState();
    expect(s.view).toBe('terminal');
    expect(s.focusedSessionId).toBe('sess-1');
    expect(s.paneAssignment[0]).toBe('sess-1');
  });

  it('view を省略した場合は terminal が既定', () => {
    useAppStore.getState().focusSession('sess-1');
    expect(useAppStore.getState().view).toBe('terminal');
  });

  it('アクティブペインが 1 のときはペイン 1 に割り当て、ペイン 0 を壊さない', () => {
    useAppStore.setState({ activePane: 1, paneAssignment: ['sess-a', null] });

    useAppStore.getState().focusSession('sess-b', 'terminal');

    const s = useAppStore.getState();
    expect(s.paneAssignment).toEqual(['sess-a', 'sess-b']);
    expect(s.focusedSessionId).toBe('sess-b');
  });

  it('既に別ペインに割り当て済みのセッションを再フォーカスしても重複しない', () => {
    useAppStore.setState({ activePane: 0, paneAssignment: ['sess-a', 'sess-b'] });

    useAppStore.getState().focusSession('sess-b', 'terminal');

    const s = useAppStore.getState();
    expect(s.paneAssignment).toEqual(['sess-b', null]);
    expect(s.focusedSessionId).toBe('sess-b');
  });

  it('エディタ画面へのフォーカスもできる（M3-1 が使う）', () => {
    useAppStore.getState().focusSession('sess-1', 'editor');
    expect(useAppStore.getState().view).toBe('editor');
    expect(useAppStore.getState().focusedSessionId).toBe('sess-1');
  });
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
