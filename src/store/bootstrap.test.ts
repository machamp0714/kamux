import { beforeEach, describe, expect, it, vi } from 'vitest';

const listProjects = vi.fn();
const listSessions = vi.fn();
vi.mock('../ipc/commands', () => ({
  listProjects: (...args: unknown[]) => listProjects(...args),
  listSessions: (...args: unknown[]) => listSessions(...args),
  createProject: vi.fn(),
  createSession: vi.fn(),
  updateSession: vi.fn(),
}));

import type { Project, Session } from '../types/model';
import { bootstrap, useAppStore } from './index';
import { ACTIVE_PROJECT_STORAGE_KEY } from './projectSlice';
import { emptySessionOrder } from './sessionSlice';

const project = (id: string): Project => ({
  id,
  name: `name-${id}`,
  repo_path: `/repo-path/${id}`,
  default_cli: 'claude',
  created_at: 1,
  updated_at: 1,
});

beforeEach(() => {
  listProjects.mockReset();
  listSessions.mockReset().mockResolvedValue([]);
  localStorage.clear();
  useAppStore.setState({
    projects: [],
    activeProjectId: null,
    sessions: {},
    sessionOrder: emptySessionOrder(),
  });
});

describe('bootstrap', () => {
  it('保存された ID のプロジェクトを復元する（再起動後の復帰）', async () => {
    listProjects.mockResolvedValue([project('p1'), project('p2')]);
    localStorage.setItem(ACTIVE_PROJECT_STORAGE_KEY, 'p2');

    await bootstrap();

    expect(useAppStore.getState().activeProjectId).toBe('p2');
    expect(listSessions).toHaveBeenCalledWith('p2', false);
  });

  it('保存が無ければ先頭のプロジェクトを選ぶ', async () => {
    listProjects.mockResolvedValue([project('p1'), project('p2')]);

    await bootstrap();

    expect(useAppStore.getState().activeProjectId).toBe('p1');
    expect(listSessions).toHaveBeenCalledWith('p1', false);
  });

  it('保存された ID が消えたプロジェクトを指していたら先頭に落とす', async () => {
    listProjects.mockResolvedValue([project('p1')]);
    localStorage.setItem(ACTIVE_PROJECT_STORAGE_KEY, 'deleted');

    await bootstrap();

    expect(useAppStore.getState().activeProjectId).toBe('p1');
  });

  it('プロジェクトが 1 件も無ければ何も選ばず、セッションも読まない', async () => {
    listProjects.mockResolvedValue([]);

    await bootstrap();

    expect(useAppStore.getState().activeProjectId).toBeNull();
    expect(listSessions).not.toHaveBeenCalled();
  });

  it('セッションの並びが sort_order どおりに復元される', async () => {
    listProjects.mockResolvedValue([project('p1')]);
    listSessions.mockResolvedValue([
      {
        id: 'b',
        project_id: 'p1',
        title: 'b',
        description: '',
        kanban_status: 'backlog',
        sort_order: 2,
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
      },
      {
        id: 'a',
        project_id: 'p1',
        title: 'a',
        description: '',
        kanban_status: 'backlog',
        sort_order: 1,
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
      },
    ]);

    await bootstrap();

    expect(useAppStore.getState().sessionOrder.backlog).toEqual(['a', 'b']);
  });

  it('セッション読み込みが終わるまで bootstrap は解決しない（await 落ちの検出）', async () => {
    listProjects.mockResolvedValue([project('p1')]);
    let resolveSessions: (v: Session[]) => void = () => {};
    listSessions.mockReturnValue(
      new Promise<Session[]>((resolve) => {
        resolveSessions = resolve;
      }),
    );

    let settled = false;
    const done = bootstrap().then(() => {
      settled = true;
    });

    // マクロタスクを 1 周させ、保留中のマイクロタスクをすべて流し切る。
    // await が抜けていると（= setActiveProject の完了を待たずに戻ると）
    // ここまでに settled が true になってしまう。
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(settled).toBe(false);

    resolveSessions([]);
    await done;
    expect(settled).toBe(true);
  });
});
