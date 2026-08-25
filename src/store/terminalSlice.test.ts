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
  isScratch = false,
  sortOrder = 1,
): Session {
  return {
    id,
    project_id: 'p1',
    title: id,
    description: '',
    kanban_status: kanbanStatus,
    sort_order: sortOrder,
    mode: 'in_place',
    branch: null,
    worktree_path: null,
    cli_kind: 'shell',
    cli_command: null,
    claude_session_id: null,
    last_runtime_state: 'idle',
    last_runtime_error: null,
    first_started_at: 1,
    heuristics_enabled: true,
    silence_timeout_secs: 30,
    is_scratch: isScratch,
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

  // Ruling 20-G（lane-controller、契約 §29.7）: sessionOrder は scratch を含まない
  // 設計（契約 §29.4）だが、供給源がそれしか無いと SCRATCH グループが production で
  // 恒久的に空になる。sessions から is_scratch === true && archived_at === null を
  // 直接引いて連結する。フィクスチャは sessionOrder に scratch の id を置かない
  // （production が到達できる状態で測る）。
  it('SCRATCH セッションは sessionOrder ではなく sessions から直接引いて末尾に連結する（契約 §29.7 / Ruling 20-G）', () => {
    const order = emptyOrder();
    order.backlog = ['b1'];
    const store = makeStore(
      [session('b1', 'backlog'), session('scratch1', 'backlog', null, true)],
      order,
    );
    expect(selectTerminalTabs(store.getState())).toEqual(['b1', 'scratch1']);
  });

  it('archived な scratch は含めない', () => {
    const order = emptyOrder();
    const store = makeStore([session('s1', 'backlog', 1000, true)], order);
    expect(selectTerminalTabs(store.getState())).toEqual([]);
  });

  it('複数の SCRATCH は sort_order 昇順、同値なら id 辞書順で並ぶ（buildSessionOrder と同じタイブレーク）', () => {
    const order = emptyOrder();
    const store = makeStore(
      [
        session('z', 'backlog', null, true, 2),
        session('a', 'backlog', null, true, 2),
        session('m', 'backlog', null, true, 1),
      ],
      order,
    );
    expect(selectTerminalTabs(store.getState())).toEqual(['m', 'a', 'z']);
  });

  // 自己設計の変異検証（advisor 指摘）: sessionOrder.ts の buildSessionOrder（loadSessions が
  // 使う実体）は is_scratch を絞らないため、プロジェクト再訪後は scratch の id が
  // sessionOrder.backlog に紛れ込みうる（sessionSlice.ts の loadSessions →
  // projectSlice.ts の setActiveProject 経由。§29.4 の境界そのものである
  // sessionOrder.ts / kanbanOrder.ts 自体は変更しない）。この状態でも id が重複しないこと。
  it('sessionOrder に scratch の id が紛れ込んでいても重複させない', () => {
    const order = emptyOrder();
    order.backlog = ['scratch1'];
    const store = makeStore([session('scratch1', 'backlog', null, true)], order);
    expect(selectTerminalTabs(store.getState())).toEqual(['scratch1']);
  });
});

describe('assignPane', () => {
  it('ペインにセッションを割り当て、そのペインをアクティブにしてフォーカスも移す', () => {
    const order = emptyOrder();
    order.backlog = ['b1'];
    const store = makeStore([session('b1', 'backlog')], order);
    // single では pane 引数を無視して activePane に寄せる（paneLogic.ts の
    // assignPaneReducer、設計 §3.5）。pane 1 を明示的に指定する意図を保つため
    // split2 にしてから呼ぶ。
    store.getState().setLayout('split2');
    store.getState().assignPane(1, 'b1');
    expect(store.getState().paneAssignment).toEqual([null, 'b1']);
    expect(store.getState().activePane).toBe(1);
    expect(store.getState().focusedSessionId).toBe('b1');
  });

  it('single では pane 引数を無視して activePane に寄せる（reducer 単体の観測は paneLogic.test.ts:94。ここは terminalSlice 経由の配線の統合確認）', () => {
    const order = emptyOrder();
    order.backlog = ['a'];
    const store = makeStore([session('a', 'backlog')], order);
    store.getState().assignPane(1, 'a');
    const s = store.getState();
    expect(s.paneAssignment).toEqual(['a', null]);
    expect(s.activePane).toBe(0);
    expect(s.focusedSessionId).toBe('a');
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
    // single では pane 引数を無視して activePane に寄せる（paneLogic.ts の
    // assignPaneReducer、設計 §3.5）。pane 1 をアクティブにする意図を保つため
    // split2 にしてから呼ぶ。
    store.getState().setLayout('split2');
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
    expect(store.getState().focusedSessionId).toBeNull();
  });
});

// --- ここから M3-2 Task 6 の追加分。既存の describe は変更しない ---

describe('setActivePane', () => {
  it('split2 でアクティブペインを切り替え、focusedSessionId を同期する', () => {
    const order = emptyOrder();
    order.backlog = ['a', 'b'];
    const store = makeStore([session('a', 'backlog'), session('b', 'backlog')], order);
    store.getState().setLayout('split2');
    store.getState().assignPane(0, 'a');
    store.getState().assignPane(1, 'b');
    store.getState().setActivePane(0);
    expect(store.getState().activePane).toBe(0);
    expect(store.getState().focusedSessionId).toBe('a');
  });

  it('single では setActivePane が no-op で focusedSessionId も動かない', () => {
    const order = emptyOrder();
    order.backlog = ['a'];
    const store = makeStore([session('a', 'backlog')], order);
    store.getState().assignPane(0, 'a');
    store.getState().setActivePane(1);
    const s = store.getState();
    expect(s.activePane).toBe(0);
    expect(s.paneAssignment).toEqual(['a', null]);
    expect(s.focusedSessionId).toBe('a');
  });

  it('single に戻っても activePane=1 を維持し、setActivePane は依然 no-op（片方向フィクスチャの穴を塞ぐ）', () => {
    const order = emptyOrder();
    order.backlog = ['a', 'b'];
    const store = makeStore([session('a', 'backlog'), session('b', 'backlog')], order);
    store.getState().setLayout('split2');
    store.getState().assignPane(0, 'a');
    store.getState().assignPane(1, 'b');
    store.getState().setActivePane(1);
    store.getState().setLayout('single');
    expect(store.getState().activePane).toBe(1);
    expect(store.getState().focusedSessionId).toBe('b');

    store.getState().setActivePane(0);
    expect(store.getState().activePane).toBe(1);
    expect(store.getState().focusedSessionId).toBe('b');
  });
});

describe('resetTerminalLayout', () => {
  it('初期状態へ戻し focusedSessionId も null にする', () => {
    const order = emptyOrder();
    order.backlog = ['a'];
    const store = makeStore([session('a', 'backlog')], order);
    store.getState().setLayout('split2');
    store.getState().assignPane(0, 'a');
    store.getState().resetTerminalLayout();
    const s = store.getState();
    expect(s.layout).toBe('single');
    expect(s.paneAssignment).toEqual([null, null]);
    expect(s.activePane).toBe(0);
    expect(s.focusedSessionId).toBeNull();
  });
});

describe('setLayout はペイン割当を保持する', () => {
  it('single ↔ split2 を往復しても paneAssignment は消えない', () => {
    const order = emptyOrder();
    order.backlog = ['a', 'b'];
    const store = makeStore([session('a', 'backlog'), session('b', 'backlog')], order);
    store.getState().setLayout('split2');
    store.getState().assignPane(0, 'a');
    store.getState().assignPane(1, 'b');
    store.getState().setLayout('single');
    expect(store.getState().paneAssignment).toEqual(['a', 'b']);
    store.getState().setLayout('split2');
    expect(store.getState().paneAssignment).toEqual(['a', 'b']);
  });
});

describe('split2-v でもペイン対応が機能する（契約 §28.1／§28.6）', () => {
  it('assignPane と setActivePane が focusedSessionId を同期する', () => {
    const order = emptyOrder();
    order.backlog = ['a', 'b'];
    const store = makeStore([session('a', 'backlog'), session('b', 'backlog')], order);
    store.getState().setLayout('split2-v');
    store.getState().assignPane(0, 'a');
    store.getState().assignPane(1, 'b');
    store.getState().setActivePane(1);
    expect(store.getState().activePane).toBe(1);
    expect(store.getState().focusedSessionId).toBe('b');
  });
});

describe('withFocus は set() へ渡す形を 4 フィールドに保つ（PR 25 判定事項・変異 #6 の観測強化）', () => {
  it('変化の無い setLayout 呼び出しでも set() の引数は 4 キーちょうど', () => {
    const calls: Array<Record<string, unknown>> = [];
    const store = create<TestStore>()((set, get, api) => {
      const spySet: typeof set = (partial, replace) => {
        if (typeof partial !== 'function') {
          calls.push(partial as Record<string, unknown>);
        }
        set(partial as Parameters<typeof set>[0], replace);
      };
      return {
        sessions: {},
        sessionOrder: emptyOrder(),
        focusedSessionId: null,
        ...(createTerminalSlice as unknown as StateCreator<TestStore, [], [], TerminalSlice>)(
          spySet,
          get,
          api,
        ),
      };
    });

    // layout は初期値のまま 'single' なので、この呼び出しは reducer の早期 return
    // （no-op）経路を通る。ここで reducer に渡した p は get() そのもの
    // （sessions / sessionOrder など terminalSlice 外のフィールドも持つ AppStore 全体）
    // なので、withFocus が `{ ...p, focusedSessionId }` の spread になっていれば
    // set() へ渡る partial のキー数がここで膨らむ。
    store.getState().setLayout('single');

    expect(calls).toHaveLength(1);
    expect(Object.keys(calls[0]).sort()).toEqual(
      ['activePane', 'focusedSessionId', 'layout', 'paneAssignment'].sort(),
    );
  });
});
