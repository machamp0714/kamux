import { beforeEach, describe, expect, it, vi } from 'vitest';

const updateSession = vi.fn();
const listSessions = vi.fn();
vi.mock('../ipc/commands', () => ({
  updateSession: (...args: unknown[]) => updateSession(...args),
  listSessions: (...args: unknown[]) => listSessions(...args),
  createSession: vi.fn(),
  listProjects: vi.fn(),
  createProject: vi.fn(),
}));

import type { Session } from '../types/model';
import { useAppStore } from './index';
import { computeSortOrder, emptySessionOrder } from './sessionSlice';

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
  useAppStore.setState({ sessions: {}, sessionOrder: emptySessionOrder() });
});

describe('computeSortOrder', () => {
  it('空の列に入れると 1', () => {
    expect(computeSortOrder([], 0)).toBe(1);
  });

  it('先頭に入れると最小 - 1', () => {
    expect(computeSortOrder([3, 4], 0)).toBe(2);
  });

  it('末尾に入れると最大 + 1', () => {
    expect(computeSortOrder([1, 2], 2)).toBe(3);
    // 「最大 + 1」であることを、末尾の値そのものや配列長と区別できる値で確認する
    expect(computeSortOrder([5, 10], 2)).toBe(11);
  });

  it('間に入れると両隣の中点', () => {
    expect(computeSortOrder([1, 2], 1)).toBe(1.5);
    expect(computeSortOrder([1, 1.5], 1)).toBe(1.25);
  });

  it('負の値でも中点規則が成り立つ', () => {
    expect(computeSortOrder([-2, 0], 1)).toBe(-1);
    expect(computeSortOrder([-2], 0)).toBe(-3);
  });
});

describe('moveCard', () => {
  const seed = () => {
    useAppStore.setState({
      sessions: {
        a: session('a', 'backlog', 1),
        b: session('b', 'backlog', 2),
        c: session('c', 'in_progress', 1),
      },
      sessionOrder: { backlog: ['a', 'b'], in_progress: ['c'], review: [], done: [] },
    });
  };

  it('別の列へ移すと両方の列の並びが更新される', async () => {
    seed();
    updateSession.mockResolvedValue(session('a', 'in_progress', 0));

    await useAppStore.getState().moveCard('a', 'in_progress', 0);

    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['b']);
    expect(useAppStore.getState().sessionOrder.in_progress).toEqual(['a', 'c']);
    expect(updateSession).toHaveBeenCalledWith('a', {
      kanban_status: 'in_progress',
      sort_order: 0,
    });
  });

  it('同じ列の中で末尾へ並べ替えると、自分を除いた最大 + 1 になる', async () => {
    useAppStore.setState({
      sessions: {
        a: session('a', 'backlog', 1),
        b: session('b', 'backlog', 2),
        c: session('c', 'backlog', 3),
      },
      sessionOrder: { backlog: ['a', 'b', 'c'], in_progress: [], review: [], done: [] },
    });
    updateSession.mockResolvedValue(session('a', 'backlog', 2.5));

    await useAppStore.getState().moveCard('a', 'backlog', 2);

    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['b', 'c', 'a']);
    expect(updateSession).toHaveBeenCalledWith('a', { kanban_status: 'backlog', sort_order: 4 });
  });

  it('同じ列の中で中間へ並べ替えると、両隣の中点になる', async () => {
    useAppStore.setState({
      sessions: {
        a: session('a', 'backlog', 1),
        b: session('b', 'backlog', 2),
        c: session('c', 'backlog', 3),
      },
      sessionOrder: { backlog: ['a', 'b', 'c'], in_progress: [], review: [], done: [] },
    });
    // 移動対象を 'a'（先頭）にすることで、自分自身を neighbors から除外しているかを
    // 「たまたま同じ中点になる」ケース（'c' を動かす場合）と区別できるようにする。
    updateSession.mockResolvedValue(session('a', 'backlog', 2.5));

    await useAppStore.getState().moveCard('a', 'backlog', 1);

    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['b', 'a', 'c']);
    expect(updateSession).toHaveBeenCalledWith('a', { kanban_status: 'backlog', sort_order: 2.5 });
  });

  it('空の列に移すと sort_order は 1 になる', async () => {
    seed();
    updateSession.mockResolvedValue(session('a', 'review', 9));

    await useAppStore.getState().moveCard('a', 'review', 0);

    expect(updateSession).toHaveBeenCalledWith('a', { kanban_status: 'review', sort_order: 1 });
  });

  it('サーバが返した行でストアを確定させる', async () => {
    seed();
    updateSession.mockResolvedValue({ ...session('a', 'review', 9), title: 'server wins' });

    await useAppStore.getState().moveCard('a', 'review', 0);

    expect(useAppStore.getState().sessions.a.title).toBe('server wins');
    expect(useAppStore.getState().sessions.a.sort_order).toBe(9);
  });

  it('存在しない id なら何もしない', async () => {
    seed();
    await useAppStore.getState().moveCard('nope', 'done', 0);
    expect(updateSession).not.toHaveBeenCalled();
    expect(useAppStore.getState().sessionOrder.done).toEqual([]);
  });

  it('保存に失敗したら楽観更新を巻き戻してからエラーを投げる', async () => {
    seed();
    const before = useAppStore.getState().sessionOrder;
    updateSession.mockRejectedValue({ code: 'db', message: 'disk I/O error' });

    await expect(useAppStore.getState().moveCard('a', 'done', 0)).rejects.toMatchObject({
      code: 'db',
    });

    expect(useAppStore.getState().sessionOrder).toEqual(before);
    expect(useAppStore.getState().sessions.a.kanban_status).toBe('backlog');
    expect(useAppStore.getState().sessions.a.sort_order).toBe(1);
  });
});
