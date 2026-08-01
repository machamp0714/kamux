import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session, SessionPatch } from '../types/model';
import { useAppStore } from './index';
import { buildSessionOrder } from './kanbanOrder';

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
});
