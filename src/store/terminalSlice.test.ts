import { describe, expect, it } from 'vitest';
import { create, type StateCreator } from 'zustand';
import {
  createTerminalSlice,
  selectTerminalTabs,
  TAB_COLUMN_ORDER,
  type TerminalSlice,
} from './terminalSlice';
import type { AppStore } from './index';
import type { KanbanStatus, Session } from '../types/model';

function session(
  id: string,
  kanbanStatus: KanbanStatus,
  archivedAt: number | null = null,
): Session {
  return {
    id,
    project_id: 'p1',
    title: id,
    description: '',
    kanban_status: kanbanStatus,
    sort_order: 1,
    mode: 'in_place',
    branch: null,
    worktree_path: null,
    cli_kind: 'shell',
    cli_command: null,
    claude_session_id: null,
    last_runtime_state: 'idle',
    last_runtime_error: null,
    first_started_at: 1,
    archived_at: archivedAt,
    created_at: 0,
    updated_at: 0,
  };
}

type TestStore = Pick<AppStore, 'sessions' | 'sessionOrder' | 'focusedSessionId'> & TerminalSlice;

function makeStore(sessions: Session[], sessionOrder: Record<KanbanStatus, string[]>) {
  return create<TestStore>()((set, get, api) => ({
    sessions: Object.fromEntries(sessions.map((s) => [s.id, s])),
    sessionOrder,
    focusedSessionId: null,
    ...(createTerminalSlice as unknown as StateCreator<TestStore, [], [], TerminalSlice>)(
      set,
      get,
      api,
    ),
  }));
}

const emptyOrder = (): Record<KanbanStatus, string[]> => ({
  backlog: [],
  in_progress: [],
  review: [],
  done: [],
});

describe('selectTerminalTabs', () => {
  it('列順は in_progress → backlog → review → done', () => {
    expect(TAB_COLUMN_ORDER).toEqual(['in_progress', 'backlog', 'review', 'done']);
    const order = emptyOrder();
    order.backlog = ['b1'];
    order.in_progress = ['i1', 'i2'];
    order.done = ['d1'];
    const store = makeStore(
      [
        session('b1', 'backlog'),
        session('i1', 'in_progress'),
        session('i2', 'in_progress'),
        session('d1', 'done'),
      ],
      order,
    );
    expect(selectTerminalTabs(store.getState())).toEqual(['i1', 'i2', 'b1', 'd1']);
  });

  it('アーカイブ済みと欠損セッションを除外する', () => {
    const order = emptyOrder();
    order.backlog = ['b1', 'archived', 'ghost'];
    const store = makeStore(
      [session('b1', 'backlog'), session('archived', 'backlog', 1000)],
      order,
    );
    expect(selectTerminalTabs(store.getState())).toEqual(['b1']);
  });

  it('列内は sessionOrder の順序をそのまま保ち、再ソートしない', () => {
    const order = emptyOrder();
    // 2 件とも sort_order は同じ 1 だが、id の辞書順とは逆に並べて渡す。
    order.backlog = ['zeta', 'alpha'];
    const store = makeStore([session('alpha', 'backlog'), session('zeta', 'backlog')], order);
    expect(selectTerminalTabs(store.getState())).toEqual(['zeta', 'alpha']);
  });
});

describe('assignPane', () => {
  it('ペインにセッションを割り当て、そのペインをアクティブにしてフォーカスも移す', () => {
    const order = emptyOrder();
    order.backlog = ['b1'];
    const store = makeStore([session('b1', 'backlog')], order);
    store.getState().assignPane(1, 'b1');
    expect(store.getState().paneAssignment).toEqual([null, 'b1']);
    expect(store.getState().activePane).toBe(1);
    expect(store.getState().focusedSessionId).toBe('b1');
  });
});

describe('cycleSession', () => {
  const order = () => {
    const o = emptyOrder();
    o.in_progress = ['i1'];
    o.backlog = ['b1', 'b2'];
    return o;
  };
  const sessions = () => [
    session('i1', 'in_progress'),
    session('b1', 'backlog'),
    session('b2', 'backlog'),
  ];

  it('未選択のとき dir=1 は先頭を選ぶ', () => {
    const store = makeStore(sessions(), order());
    store.getState().cycleSession(1);
    expect(store.getState().paneAssignment[0]).toBe('i1');
  });

  it('未選択のとき dir=-1 は末尾を選ぶ', () => {
    const store = makeStore(sessions(), order());
    store.getState().cycleSession(-1);
    expect(store.getState().paneAssignment[0]).toBe('b2');
  });

  it('末尾から dir=1 で先頭に巻き戻る', () => {
    const store = makeStore(sessions(), order());
    store.getState().assignPane(0, 'b2');
    store.getState().cycleSession(1);
    expect(store.getState().paneAssignment[0]).toBe('i1');
  });

  it('先頭から dir=-1 で末尾に巻き戻る', () => {
    const store = makeStore(sessions(), order());
    store.getState().assignPane(0, 'i1');
    store.getState().cycleSession(-1);
    expect(store.getState().paneAssignment[0]).toBe('b2');
  });

  it('タブが無いときは何もしない', () => {
    const store = makeStore([], emptyOrder());
    store.getState().cycleSession(1);
    expect(store.getState().paneAssignment).toEqual([null, null]);
  });

  it('アクティブなペインだけを動かす', () => {
    const store = makeStore(sessions(), order());
    store.getState().assignPane(1, 'i1');
    store.getState().cycleSession(1);
    expect(store.getState().paneAssignment).toEqual([null, 'b1']);
  });
});

describe('setLayout', () => {
  it('single と split2 を切り替える', () => {
    const store = makeStore([], emptyOrder());
    expect(store.getState().layout).toBe('single');
    store.getState().setLayout('split2');
    expect(store.getState().layout).toBe('split2');
  });
});
