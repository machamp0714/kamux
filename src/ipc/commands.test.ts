import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import {
  createProject,
  createSession,
  listProjects,
  listSessions,
  updateSession,
} from './commands';

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(null);
});

describe('ipc/commands', () => {
  it('createProject が契約どおりのコマンド名と camelCase 引数で invoke する', async () => {
    await createProject('kamux', '/Users/x/repo/kamux', 'claude');
    expect(invoke).toHaveBeenCalledWith('create_project', {
      name: 'kamux',
      repoPath: '/Users/x/repo/kamux',
      defaultCli: 'claude',
    });
  });

  it('listProjects が引数なしで invoke する', async () => {
    await listProjects();
    expect(invoke).toHaveBeenCalledWith('list_projects');
  });

  it('createSession が 7 引数すべてを camelCase で渡す', async () => {
    await createSession({
      projectId: 'p1',
      title: 'fix login',
      description: '',
      mode: 'worktree',
      branch: 'session/fix-login',
      cliKind: 'claude',
      cliCommand: null,
    });
    expect(invoke).toHaveBeenCalledWith('create_session', {
      projectId: 'p1',
      title: 'fix login',
      description: '',
      mode: 'worktree',
      branch: 'session/fix-login',
      cliKind: 'claude',
      cliCommand: null,
    });
  });

  it('updateSession の patch は snake_case のまま渡す（Tauri の自動変換は引数名だけに効く）', async () => {
    await updateSession('s1', { kanban_status: 'review', sort_order: 1.5 });
    expect(invoke).toHaveBeenCalledWith('update_session', {
      id: 's1',
      patch: { kanban_status: 'review', sort_order: 1.5 },
    });
  });

  it('updateSession は archived_at の null を落とさずに渡す', async () => {
    await updateSession('s1', { archived_at: null });
    expect(invoke).toHaveBeenCalledWith('update_session', {
      id: 's1',
      patch: { archived_at: null },
    });
  });

  it('listSessions が projectId と includeArchived を渡す', async () => {
    await listSessions('p1', false);
    expect(invoke).toHaveBeenCalledWith('list_sessions', {
      projectId: 'p1',
      includeArchived: false,
    });
  });
});
