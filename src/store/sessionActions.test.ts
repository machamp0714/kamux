import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session, SessionPatch } from '../types/model';
import { useAppStore } from './index';
import { buildSessionOrder, emptySessionOrder } from './kanbanOrder';

vi.mock('../ipc/commands', () => ({
  createProject: vi.fn(),
  listProjects: vi.fn(),
  createSession: vi.fn(),
  updateSession: vi.fn(),
  listSessions: vi.fn(),
  moveSession: vi.fn(),
}));

import { updateSession } from '../ipc/commands';

function makeSession(overrides: Partial<Session> & { id: string }): Session {
  return {
    project_id: 'p1',
    title: overrides.id,
    description: '',
    kanban_status: 'backlog',
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
    archived_at: null,
    created_at: 0,
    updated_at: 0,
    ...overrides,
  };
}

function seed(list: Session[]) {
  const sessions = Object.fromEntries(list.map((s) => [s.id, s]));
  useAppStore.setState({
    sessions,
    sessionOrder: buildSessionOrder(sessions),
    activeProjectId: 'p1',
  });
}

beforeEach(() => {
  vi.mocked(updateSession).mockReset();
});

describe('runtimeStates', () => {
  it('初期値は空（M1-2 は表示枠のみ。値の導出は M2-1）', () => {
    expect(useAppStore.getState().runtimeStates).toEqual({});
  });
});

describe('editSession', () => {
  it('patch をそのまま送り、戻り値でセッションを置き換える', async () => {
    seed([makeSession({ id: 'a', title: '旧' })]);
    vi.mocked(updateSession).mockResolvedValue(makeSession({ id: 'a', title: '新' }));

    const saved = await useAppStore.getState().editSession('a', { title: '新' });

    expect(updateSession).toHaveBeenCalledWith('a', { title: '新' });
    expect(saved.title).toBe('新');
    expect(useAppStore.getState().sessions.a.title).toBe('新');
  });

  it('title だけ変えても列の並びは動かない', async () => {
    seed([
      makeSession({ id: 'a', kanban_status: 'review', sort_order: 1 }),
      makeSession({ id: 'b', kanban_status: 'review', sort_order: 2 }),
    ]);
    vi.mocked(updateSession).mockResolvedValue(
      makeSession({ id: 'a', kanban_status: 'review', sort_order: 1, title: '新' }),
    );

    await useAppStore.getState().editSession('a', { title: '新' });

    expect(useAppStore.getState().sessionOrder.review).toEqual(['a', 'b']);
  });

  it('失敗したら rethrow し、ストアを変えない', async () => {
    seed([makeSession({ id: 'a', title: '旧' })]);
    vi.mocked(updateSession).mockRejectedValue({ code: 'not_found', message: 'a' });

    await expect(useAppStore.getState().editSession('a', { title: '新' })).rejects.toEqual({
      code: 'not_found',
      message: 'a',
    });
    expect(useAppStore.getState().sessions.a.title).toBe('旧');
  });

  it('応答が返るまでにプロジェクトが切り替わっていたら、B の sessions へ A の応答を混ぜない（Task 19 の不変条件）', async () => {
    seed([makeSession({ id: 'a', title: '旧' })]); // activeProjectId: 'p1'
    const bSessions = { b: makeSession({ id: 'b', project_id: 'p2' }) };
    vi.mocked(updateSession).mockImplementation(async (id, patch: SessionPatch) => {
      const saved = { ...useAppStore.getState().sessions[id], ...patch } as Session;
      // setActiveProject → loadSessions の実経路がそうするように、切り替えは
      // activeProjectId の更新と sessions / sessionOrder の丸ごと置き換えを伴う
      useAppStore.setState({
        activeProjectId: 'p2',
        sessions: bSessions,
        sessionOrder: buildSessionOrder(bSessions),
      });
      return saved;
    });

    const saved = await useAppStore.getState().editSession('a', { title: '新' });

    // IPC 自体は成功しているので戻り値は呼び出し元へそのまま返す
    expect(saved.title).toBe('新');
    // だが stale なプロジェクト(p1)の応答を B の sessions へ適用してはいけない
    expect(useAppStore.getState().sessions).toEqual(bSessions);
  });
});

describe('archiveSession', () => {
  it('archived_at に数値を書き、盤面から消す', async () => {
    seed([
      makeSession({ id: 'a', kanban_status: 'done', sort_order: 1 }),
      makeSession({ id: 'b', kanban_status: 'done', sort_order: 2 }),
    ]);
    vi.mocked(updateSession).mockImplementation(
      async (id, patch: SessionPatch) =>
        ({
          ...useAppStore.getState().sessions[id],
          ...patch,
        }) as Session,
    );

    await useAppStore.getState().archiveSession('a');

    const [id, patch] = vi.mocked(updateSession).mock.calls[0];
    expect(id).toBe('a');
    expect(typeof patch.archived_at).toBe('number');
    expect(Object.keys(patch)).toEqual(['archived_at']);
    expect(useAppStore.getState().sessionOrder.done).toEqual(['b']);
  });

  it('IPC を await する前に盤面から消す（楽観的更新）', async () => {
    seed([makeSession({ id: 'a', kanban_status: 'done', sort_order: 1 })]);
    let orderDuringIpc: string[] = [];
    vi.mocked(updateSession).mockImplementation(async (id, patch: SessionPatch) => {
      orderDuringIpc = [...useAppStore.getState().sessionOrder.done];
      return { ...useAppStore.getState().sessions[id], ...patch } as Session;
    });

    await useAppStore.getState().archiveSession('a');

    expect(orderDuringIpc).toEqual([]);
  });

  it('失敗したらカードを盤面へ戻して rethrow する', async () => {
    seed([makeSession({ id: 'a', kanban_status: 'done', sort_order: 1 })]);
    vi.mocked(updateSession).mockRejectedValue({ code: 'db', message: 'locked' });

    await expect(useAppStore.getState().archiveSession('a')).rejects.toEqual({
      code: 'db',
      message: 'locked',
    });
    expect(useAppStore.getState().sessions.a.archived_at).toBeNull();
    expect(useAppStore.getState().sessionOrder.done).toEqual(['a']);
  });

  it('存在しないセッションでは何もしない', async () => {
    seed([]);
    await useAppStore.getState().archiveSession('missing');
    expect(updateSession).not.toHaveBeenCalled();
  });

  describe('プロジェクト切り替え中の応答（Task 19 の不変条件を archiveSession でも守る）', () => {
    const bSessions = {
      b: makeSession({ id: 'b', project_id: 'p2', kanban_status: 'done', sort_order: 1 }),
    };
    function switchToB() {
      useAppStore.setState({
        activeProjectId: 'p2',
        sessions: bSessions,
        sessionOrder: buildSessionOrder(bSessions),
      });
    }

    it('成功応答が返るまでに切り替わっていたら、B の sessions へ A の応答を混ぜない', async () => {
      seed([makeSession({ id: 'a', kanban_status: 'done', sort_order: 1 })]);
      vi.mocked(updateSession).mockImplementation(async (id, patch: SessionPatch) => {
        const saved = { ...useAppStore.getState().sessions[id], ...patch } as Session;
        switchToB();
        return saved;
      });

      await useAppStore.getState().archiveSession('a');

      expect(useAppStore.getState().sessions).toEqual(bSessions);
    });

    it('失敗し、かつ応答までに切り替わっていたら、B の盤面へロールバックしない。ただし throw はする', async () => {
      seed([makeSession({ id: 'a', kanban_status: 'done', sort_order: 1 })]);
      vi.mocked(updateSession).mockImplementation(async () => {
        switchToB();
        throw { code: 'db', message: 'locked' };
      });

      await expect(useAppStore.getState().archiveSession('a')).rejects.toEqual({
        code: 'db',
        message: 'locked',
      });

      // A の盤面(a を含む)へロールバックされていないこと。B の盤面のまま
      expect(useAppStore.getState().sessions).toEqual(bSessions);
      expect(useAppStore.getState().sessionOrder).toEqual(buildSessionOrder(bSessions));
    });
  });
});

describe('局所更新であることの判別テスト（buildSessionOrder による全列再構築との区別）', () => {
  /**
   * seed() は sessionOrder を buildSessionOrder(sessions) から作るため、局所更新でも
   * 全列再構築でも sessionOrder が最初から sort_order 昇順と一致してしまい、
   * どちらの実装でも同じ結果になって区別できない（PR 5 のレビューが指摘したのと
   * 同型の穴）。ここでは seed() を使わず、sessionOrder が sort_order の昇順と
   * わざと食い違う状態を直接 setState で再現する。これは架空の状態ではなく、
   * moveCard の楽観更新（契約 §7.4）が in-flight 中に残す実在の状態そのもの——
   * moveCard は列の並べ替えだけを行い、sort_order の確定値はサーバ応答を待つため、
   * 応答が返るまでは sessionOrder と sort_order がずれたままになる。
   *
   * editSession / archiveSession を将来 buildSessionOrder ベースへ「単純化」すると、
   * このずれが全列再構築で矯正されてしまい、moveCard の in-flight 中にカードが
   * 古い sort_order の位置へ吸着して見える不具合が戻る。このテストはそれを防ぐ。
   */
  function seedStale(sessions: Session[], staleReviewOrder: string[]) {
    const byId = Object.fromEntries(sessions.map((s) => [s.id, s]));
    useAppStore.setState({
      sessions: byId,
      sessionOrder: { ...emptySessionOrder(), review: staleReviewOrder },
      activeProjectId: 'p1',
    });
  }

  it('editSession は sort_order 順とずれた sessionOrder を並べ直さない', async () => {
    seedStale(
      [
        makeSession({ id: 'a', kanban_status: 'review', sort_order: 2 }),
        makeSession({ id: 'b', kanban_status: 'review', sort_order: 1 }),
      ],
      ['a', 'b'], // sort_order 昇順なら ['b', 'a'] のはずが、意図的にずらしてある
    );
    vi.mocked(updateSession).mockResolvedValue(
      makeSession({ id: 'a', kanban_status: 'review', sort_order: 2, title: '新' }),
    );

    await useAppStore.getState().editSession('a', { title: '新' });

    // 全列再構築（buildSessionOrder）なら sort_order 順の ['b', 'a'] に矯正されてしまう
    expect(useAppStore.getState().sessionOrder.review).toEqual(['a', 'b']);
  });

  it('archiveSession は残ったカードの stale な並びを保ったまま対象 id だけ除く', async () => {
    seedStale(
      [
        makeSession({ id: 'a', kanban_status: 'review', sort_order: 2 }),
        makeSession({ id: 'b', kanban_status: 'review', sort_order: 1 }),
        makeSession({ id: 'c', kanban_status: 'review', sort_order: 3 }),
      ],
      ['a', 'b', 'c'], // sort_order 昇順なら ['b', 'a', 'c']
    );
    vi.mocked(updateSession).mockImplementation(
      async (id, patch: SessionPatch) =>
        ({
          ...useAppStore.getState().sessions[id],
          ...patch,
        }) as Session,
    );

    // 第 3 のカード c をアーカイブする。a・b はどちらもこのテストの操作対象ではない
    await useAppStore.getState().archiveSession('c');

    // 全列再構築なら残った a・b が sort_order 順の ['b', 'a'] に矯正されてしまう
    expect(useAppStore.getState().sessionOrder.review).toEqual(['a', 'b']);
  });
});
