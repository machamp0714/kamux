import { beforeEach, describe, expect, it, vi } from 'vitest';

const updateSession = vi.fn();
const listSessions = vi.fn();
const moveSession = vi.fn();
vi.mock('../ipc/commands', () => ({
  updateSession: (...args: unknown[]) => updateSession(...args),
  listSessions: (...args: unknown[]) => listSessions(...args),
  moveSession: (...args: unknown[]) => moveSession(...args),
  createSession: vi.fn(),
  listProjects: vi.fn(),
  createProject: vi.fn(),
}));

import type { Session } from '../types/model';
import { useAppStore } from './index';
import { emptySessionOrder } from './sessionSlice';

const session = (id: string, status: Session['kanban_status'], sortOrder: number): Session => ({
  id,
  project_id: 'p1',
  title: id,
  description: '',
  kanban_status: status,
  sort_order: sortOrder,
  mode: 'in_place',
  branch: null,
  worktree_path: null,
  cli_kind: 'shell',
  cli_command: null,
  claude_session_id: null,
  last_runtime_state: 'idle',
  last_runtime_error: null,
  first_started_at: null,
  archived_at: null,
  created_at: 1,
  updated_at: 1,
});

beforeEach(() => {
  updateSession.mockReset();
  listSessions.mockReset();
  moveSession.mockReset();
  useAppStore.setState({ sessions: {}, sessionOrder: emptySessionOrder(), activeProjectId: 'p1' });
});

describe('moveCard', () => {
  /** review 列が x(10), y(20) で埋まっていて backlog に a(1) がある盤面 */
  function seed() {
    useAppStore.setState({
      sessions: {
        a: session('a', 'backlog', 1),
        x: session('x', 'review', 10),
        y: session('y', 'review', 20),
      },
      sessionOrder: { backlog: ['a'], in_progress: [], review: ['x', 'y'], done: [] },
    });
  }

  it('move_session を id / to_status / to_index の 3 引数で 1 回だけ呼ぶ', async () => {
    seed();
    moveSession.mockResolvedValue([
      session('x', 'review', 10),
      session('a', 'review', 15),
      session('y', 'review', 20),
    ]);

    await useAppStore.getState().moveCard('a', 'review', 1);

    expect(moveSession).toHaveBeenCalledTimes(1);
    expect(moveSession).toHaveBeenCalledWith('a', 'review', 1);
    // フロントは sort_order を算出しない（契約 §7.4）
    expect(updateSession).not.toHaveBeenCalled();
  });

  it('戻り値で移動先の列を丸ごと置き換える（サーバの順を正とする）', async () => {
    seed();
    // サーバが楽観更新と違う順を返しても、サーバ側が勝つこと
    moveSession.mockResolvedValue([
      session('y', 'review', 5),
      session('x', 'review', 10),
      session('a', 'review', 15),
    ]);

    await useAppStore.getState().moveCard('a', 'review', 1);

    expect(useAppStore.getState().sessionOrder.review).toEqual(['y', 'x', 'a']);
    expect(useAppStore.getState().sessions.a.sort_order).toBe(15);
    expect(useAppStore.getState().sessions.a.kanban_status).toBe('review');
    expect(useAppStore.getState().sessions.y.sort_order).toBe(5);
  });

  it('移動元の列は戻り値に含まれないが、楽観更新の除去がそのまま残る', async () => {
    seed();
    moveSession.mockResolvedValue([
      session('x', 'review', 10),
      session('a', 'review', 15),
      session('y', 'review', 20),
    ]);

    await useAppStore.getState().moveCard('a', 'review', 1);

    expect(useAppStore.getState().sessionOrder.backlog).toEqual([]);
  });

  it('同じ列の中の移動では 1 つの列が返り、それで置き換える', async () => {
    useAppStore.setState({
      sessions: {
        x: session('x', 'review', 10),
        y: session('y', 'review', 20),
        z: session('z', 'review', 30),
      },
      sessionOrder: { backlog: [], in_progress: [], review: ['x', 'y', 'z'], done: [] },
    });
    moveSession.mockResolvedValue([
      session('y', 'review', 20),
      session('z', 'review', 30),
      session('x', 'review', 31),
    ]);

    await useAppStore.getState().moveCard('x', 'review', 2);

    expect(moveSession).toHaveBeenCalledWith('x', 'review', 2);
    expect(useAppStore.getState().sessionOrder.review).toEqual(['y', 'z', 'x']);
  });

  it('空の列へ移せる', async () => {
    seed();
    moveSession.mockResolvedValue([session('a', 'done', 1)]);

    await useAppStore.getState().moveCard('a', 'done', 0);

    expect(moveSession).toHaveBeenCalledWith('a', 'done', 0);
    expect(useAppStore.getState().sessionOrder.done).toEqual(['a']);
    expect(useAppStore.getState().sessionOrder.backlog).toEqual([]);
    // 移動元でも移動先でもない列は手を付けない（丸ごと置換に書き換えると x が消える）
    expect(useAppStore.getState().sessionOrder.review).toEqual(['x', 'y']);
    expect(useAppStore.getState().sessions.x).toEqual(session('x', 'review', 10));
  });

  it('ドロップ直後に楽観更新が反映される（サーバ応答を待たない）', async () => {
    seed();
    let resolve!: (v: Session[]) => void;
    moveSession.mockReturnValue(
      new Promise<Session[]>((r) => {
        resolve = r;
      }),
    );

    const pending = useAppStore.getState().moveCard('a', 'review', 1);

    // await していない時点で既に並べ替わっていること
    expect(useAppStore.getState().sessionOrder.review).toEqual(['x', 'a', 'y']);
    expect(useAppStore.getState().sessionOrder.backlog).toEqual([]);

    resolve([session('x', 'review', 10), session('a', 'review', 15), session('y', 'review', 20)]);
    await pending;
  });

  it('存在しない id なら何もしない', async () => {
    seed();

    await useAppStore.getState().moveCard('nope', 'review', 0);

    expect(moveSession).not.toHaveBeenCalled();
    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['a']);
  });

  it('保存に失敗したら楽観更新を巻き戻してからエラーを投げる', async () => {
    seed();
    moveSession.mockRejectedValue({ code: 'db', message: 'boom' });

    await expect(useAppStore.getState().moveCard('a', 'review', 1)).rejects.toEqual({
      code: 'db',
      message: 'boom',
    });

    expect(useAppStore.getState().sessionOrder).toEqual({
      backlog: ['a'],
      in_progress: [],
      review: ['x', 'y'],
      done: [],
    });
    expect(useAppStore.getState().sessions.a.kanban_status).toBe('backlog');
  });

  describe('プロジェクト切り替え中の応答（Task 19 の不変条件を moveCard でも守る）', () => {
    /**
     * moveSession の IPC 往復中に、ユーザーが別プロジェクト B へ切り替えた状態を再現する。
     * setActiveProject → loadSessions の実経路がそうするように、B への切り替えは
     * activeProjectId の更新と sessions / sessionOrder の丸ごと置き換えを伴う。
     * ガードが無いと、A 宛ての確定応答・ロールバックが B の盤面へ後から書き込まれ、
     * 「B が active なのに A のセッションが sessions に混じる」幽霊状態になる
     * （sessionSlice.ts:41-49 が宣言した不変条件と同型。lane-controller 裁定）。
     */
    const bBoard = {
      sessions: { z: session('z', 'review', 1) },
      sessionOrder: { backlog: [], in_progress: [], review: ['z'], done: [] },
    };
    function switchToB() {
      useAppStore.setState({ activeProjectId: 'p2', ...bBoard });
    }

    it('確定応答が返るまでに切り替わっていたら、B の盤面へ A の確定応答を適用しない', async () => {
      seed();
      moveSession.mockImplementation(async () => {
        switchToB();
        return [session('a', 'review', 15), session('x', 'review', 10), session('y', 'review', 20)];
      });

      await useAppStore.getState().moveCard('a', 'review', 1);

      expect(useAppStore.getState().sessions).toEqual(bBoard.sessions);
      expect(useAppStore.getState().sessionOrder).toEqual(bBoard.sessionOrder);
    });

    it('保存に失敗し、かつ応答までに切り替わっていたら、巻き戻さず B の盤面のまま。ただし throw はする', async () => {
      seed();
      moveSession.mockImplementation(async () => {
        switchToB();
        throw { code: 'db', message: 'boom' };
      });

      await expect(useAppStore.getState().moveCard('a', 'review', 1)).rejects.toEqual({
        code: 'db',
        message: 'boom',
      });

      // A の盤面(backlog に a、review に x, y)へロールバックされていないこと
      expect(useAppStore.getState().sessions).toEqual(bBoard.sessions);
      expect(useAppStore.getState().sessionOrder).toEqual(bBoard.sessionOrder);
    });
  });
});
