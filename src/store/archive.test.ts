import { beforeEach, describe, expect, it, vi } from 'vitest';

const updateSession = vi.fn();
vi.mock('../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../ipc/commands')>()),
  updateSession: (...a: unknown[]) => updateSession(...a),
  listSessions: vi.fn().mockResolvedValue([]),
}));

import { useAppStore } from './index';
import type { Session } from '../types/model';

const s = (over: Partial<Session> & { id: string }): Session => ({
  project_id: 'p1',
  title: over.id,
  description: '',
  kanban_status: 'done',
  sort_order: 1,
  mode: 'worktree',
  branch: null,
  worktree_path: null,
  cli_kind: 'claude',
  cli_command: null,
  claude_session_id: null,
  last_runtime_state: 'idle',
  last_runtime_error: null,
  first_started_at: 1,
  heuristics_enabled: true,
  silence_timeout_secs: 30,
  archived_at: null,
  created_at: 0,
  updated_at: 0,
  ...over,
});

describe('アーカイブと復元', () => {
  beforeEach(() => {
    updateSession.mockReset();
    useAppStore.setState({
      activeProjectId: 'p1',
      sessions: { a: s({ id: 'a' }), b: s({ id: 'b', sort_order: 2 }) },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: ['a', 'b'] },
    });
  });

  // brief 由来。既存 archiveSession に対して着手前から緑（裁定 57 / 66）。
  // 赤くなったら既存実装を変えてしまった合図である。
  it('archiveSession は archived_at に現在時刻を入れて更新する', async () => {
    const before = Date.now();
    updateSession.mockImplementation(async (_id, patch) =>
      s({ id: 'a', archived_at: patch.archived_at }),
    );

    await useAppStore.getState().archiveSession('a');

    const [id, patch] = updateSession.mock.calls[0] as [string, { archived_at: number }];
    expect(id).toBe('a');
    expect(patch.archived_at).toBeGreaterThanOrEqual(before);
  });

  it('アーカイブするとボードの列から消える', async () => {
    updateSession.mockResolvedValue(s({ id: 'a', archived_at: 1754006400000 }));

    await useAppStore.getState().archiveSession('a');

    expect(useAppStore.getState().sessionOrder.done).toEqual(['b']);
    expect(useAppStore.getState().sessions.a.archived_at).toBe(1754006400000);
  });

  it('restoreSession は archived_at: null を明示的に送る', async () => {
    useAppStore.setState({ sessions: { a: s({ id: 'a', archived_at: 1754006400000 }) } });
    updateSession.mockResolvedValue(s({ id: 'a', archived_at: null }));

    await useAppStore.getState().restoreSession('a');

    const [, patch] = updateSession.mock.calls[0] as [string, Record<string, unknown>];
    expect(Object.prototype.hasOwnProperty.call(patch, 'archived_at')).toBe(true);
    expect(patch.archived_at).toBeNull();
  });

  it('復元するとボードの列に戻る', async () => {
    useAppStore.setState({
      sessions: { a: s({ id: 'a', archived_at: 1754006400000 }) },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
    });
    updateSession.mockResolvedValue(s({ id: 'a', archived_at: null }));

    await useAppStore.getState().restoreSession('a');

    expect(useAppStore.getState().sessionOrder.done).toEqual(['a']);
  });

  // 契約 §144.6: restoreSession は archiveSession の 3 性質を鏡像で持つ。
  it('IPC を await する前に盤面へ挿入する（楽観的更新）', async () => {
    useAppStore.setState({
      sessions: { a: s({ id: 'a', archived_at: 1754006400000 }) },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
    });
    let orderDuringIpc: string[] = [];
    updateSession.mockImplementation(async (id: string, patch: Record<string, unknown>) => {
      orderDuringIpc = [...useAppStore.getState().sessionOrder.done];
      return { ...useAppStore.getState().sessions[id], ...patch } as Session;
    });

    await useAppStore.getState().restoreSession('a');

    expect(orderDuringIpc).toEqual(['a']);
  });

  it('失敗したらカードを盤面から外して rethrow する', async () => {
    useAppStore.setState({
      sessions: { a: s({ id: 'a', archived_at: 1754006400000 }) },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
    });
    updateSession.mockRejectedValue({ code: 'db', message: 'locked' });

    await expect(useAppStore.getState().restoreSession('a')).rejects.toEqual({
      code: 'db',
      message: 'locked',
    });
    expect(useAppStore.getState().sessions.a.archived_at).toBe(1754006400000);
    expect(useAppStore.getState().sessionOrder.done).toEqual([]);
  });

  it('存在しないセッションでは何もしない', async () => {
    useAppStore.setState({ sessions: {} });
    await useAppStore.getState().restoreSession('missing');
    expect(updateSession).not.toHaveBeenCalled();
  });

  describe('プロジェクト切り替え中の応答（Task 19 の不変条件を restoreSession でも守る）', () => {
    const bSessions = {
      b: s({ id: 'b', project_id: 'p2', kanban_status: 'done', sort_order: 1 }),
    };
    function switchToB() {
      useAppStore.setState({
        activeProjectId: 'p2',
        sessions: bSessions,
        sessionOrder: { backlog: [], in_progress: [], review: [], done: ['b'] },
      });
    }

    it('成功応答が返るまでに切り替わっていたら、B の sessions へ A の応答を混ぜない', async () => {
      useAppStore.setState({
        sessions: { a: s({ id: 'a', archived_at: 1754006400000 }) },
        sessionOrder: { backlog: [], in_progress: [], review: [], done: [] },
      });
      updateSession.mockImplementation(async (id: string, patch: Record<string, unknown>) => {
        const saved = { ...useAppStore.getState().sessions[id], ...patch } as Session;
        switchToB();
        return saved;
      });

      await useAppStore.getState().restoreSession('a');

      expect(useAppStore.getState().sessions).toEqual(bSessions);
    });
  });

  // 契約 §144.5 / 裁定 70: 復元は冪等でなければならない。
  it('既にボードの列に居るセッションを復元しても id が重複しない', async () => {
    useAppStore.setState({
      sessions: { a: s({ id: 'a', archived_at: null }) },
      sessionOrder: { backlog: [], in_progress: [], review: [], done: ['a'] },
    });
    updateSession.mockResolvedValue(s({ id: 'a', archived_at: null }));

    await useAppStore.getState().restoreSession('a');

    expect(useAppStore.getState().sessionOrder.done).toEqual(['a']);
  });

  // 契約 §144.7 / 裁定 72: 挿入位置は (sort_order, id) の全順序で決める。土台がその
  // 全順序とずれていても（moveCard の in-flight 中に実在する状態）、他カードの並びは
  // 1 つも動かさない。
  it('挿入位置は他カードの並びを変えず (sort_order, id) の全順序で決める', async () => {
    useAppStore.setState({
      sessions: {
        x: s({ id: 'x', kanban_status: 'done', sort_order: 3 }),
        y: s({ id: 'y', kanban_status: 'done', sort_order: 1 }),
        a: s({ id: 'a', kanban_status: 'done', sort_order: 2, archived_at: 1754006400000 }),
      },
      // sort_order 昇順なら ['y', 'x'] のはずが、意図的にずらしてある
      // （moveCard の楽観更新が in-flight 中に残す実在の状態。sessionActions.test.ts:227 と同型）。
      sessionOrder: { backlog: [], in_progress: [], review: [], done: ['x', 'y'] },
    });
    updateSession.mockResolvedValue(
      s({ id: 'a', kanban_status: 'done', sort_order: 2, archived_at: null }),
    );

    await useAppStore.getState().restoreSession('a');

    // 土台 ['x', 'y'] の相対順序はそのまま。挿入位置は「土台の中で a より
    // 全順序上前に来るべき要素の個数」で決める（土台内の並び順ではなく値で判定
    // するので、土台がずれていても決定的）。x(3) は a(2) より後、y(1) は a(2)
    // より前 → 前に来るべき要素は y の 1 個だけなので index 1 に挿入する。
    expect(useAppStore.getState().sessionOrder.done).toEqual(['x', 'a', 'y']);
  });
});
