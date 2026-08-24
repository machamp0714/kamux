import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import {
  ackPty,
  cleanupWorktree,
  createProject,
  createSession,
  getHooksDiagnostics,
  listProjects,
  listSessions,
  moveSession,
  notificationPermission,
  openNotificationSettings,
  resizePty,
  setVisibilityContext,
  spawnEditor,
  startSession,
  stopSession,
  suggestBranchName,
  updateSession,
  worktreeStatus,
  writePty,
  writePtyBytes,
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

  it('updateSession の未指定フィールドはキーごと落ちる（変更しない経路）', async () => {
    await updateSession('s1', { kanban_status: 'done' });

    const [, args] = invoke.mock.calls[0] as [string, { patch: Record<string, unknown> }];
    // hasOwnProperty を archived_at だけに限らず、patch のキー集合そのものを固定する。
    // こうしないと archived_at 以外のフィールド（title / description / sort_order /
    // heuristics_enabled / silence_timeout_secs / claude_session_id）が無条件に
    // undefined で注入されても検出できない（toHaveBeenCalledWith は undefined 値の
    // プロパティを無視するため）。
    expect(Object.keys(args.patch)).toEqual(['kanban_status']);
  });

  it('listSessions が projectId と includeArchived を渡す', async () => {
    await listSessions('p1', false);
    expect(invoke).toHaveBeenCalledWith('list_sessions', {
      projectId: 'p1',
      includeArchived: false,
    });
  });

  it('listSessions は includeArchived が true のときもそのまま渡す（ハードコード禁止）', async () => {
    await listSessions('p1', true);
    expect(invoke).toHaveBeenCalledWith('list_sessions', {
      projectId: 'p1',
      includeArchived: true,
    });
  });

  it('moveSession が id / toStatus / toIndex を camelCase で渡す', async () => {
    await moveSession('s1', 'review', 1);
    expect(invoke).toHaveBeenCalledWith('move_session', {
      id: 's1',
      toStatus: 'review',
      toIndex: 1,
    });
  });

  it('startSession が id を渡す', async () => {
    await startSession('s1');
    expect(invoke).toHaveBeenCalledWith('start_session', { id: 's1' });
  });

  it('stopSession が id を渡す', async () => {
    await stopSession('s1');
    expect(invoke).toHaveBeenCalledWith('stop_session', { id: 's1' });
  });

  it('writePty が surfaceId と data を渡す', async () => {
    await writePty('s1:agent', 'ls\n');
    expect(invoke).toHaveBeenCalledWith('write_pty', { surfaceId: 's1:agent', data: 'ls\n' });
  });

  it('writePtyBytes が surfaceId と base64 を渡す（data キーではない）', async () => {
    await writePtyBytes('s1:agent', 'AAEC');
    expect(invoke).toHaveBeenCalledWith('write_pty_bytes', {
      surfaceId: 's1:agent',
      base64: 'AAEC',
    });
  });

  it('resizePty が surfaceId / cols / rows をこの順で渡す', async () => {
    await resizePty('s1:agent', 80, 24);
    expect(invoke).toHaveBeenCalledWith('resize_pty', {
      surfaceId: 's1:agent',
      cols: 80,
      rows: 24,
    });
  });

  it('ackPty が surfaceId と seq を渡す', async () => {
    await ackPty('s1:agent', 42);
    expect(invoke).toHaveBeenCalledWith('ack_pty', { surfaceId: 's1:agent', seq: 42 });
  });

  it('suggestBranchName が projectId / title / sessionId を camelCase で渡し、素の string を返す（契約 §60.2: BranchSuggestion ではない）', async () => {
    invoke.mockResolvedValue('session/fix-login-bug');

    const got = await suggestBranchName('proj-1', 'Fix login bug', 'sess-1');

    expect(invoke).toHaveBeenCalledWith('suggest_branch_name', {
      projectId: 'proj-1',
      title: 'Fix login bug',
      sessionId: 'sess-1',
    });
    expect(got).toBe('session/fix-login-bug');
  });

  it('spawnEditor が sessionId を camelCase で渡し、surface_id を返す', async () => {
    invoke.mockResolvedValue('s1:editor');

    const got = await spawnEditor('s1');

    expect(invoke).toHaveBeenCalledWith('spawn_editor', { sessionId: 's1' });
    expect(got).toBe('s1:editor');
  });

  it('getHooksDiagnostics が引数なしで invoke し、戻り値をそのまま返す', async () => {
    const diagnostics = {
      socket_path: '/tmp/kamux-hooks-42.sock',
      listener_alive: true,
      sessions: [
        {
          session_id: 's1',
          cli_kind: 'claude',
          liveness: 'healthy',
          last_hook_at: 123,
          heuristics_active: false,
        },
      ],
    };
    invoke.mockResolvedValue(diagnostics);

    const got = await getHooksDiagnostics();

    expect(invoke).toHaveBeenCalledWith('get_hooks_diagnostics');
    expect(got).toEqual(diagnostics);
  });

  it('setVisibilityContext が view と visibleSessionIds を camelCase で渡す', async () => {
    await setVisibilityContext('kanban', ['s1', 's2']);
    expect(invoke).toHaveBeenCalledWith('set_visibility_context', {
      view: 'kanban',
      visibleSessionIds: ['s1', 's2'],
    });
  });

  it('notificationPermission が引数なしで invoke し、戻り値をそのまま返す', async () => {
    invoke.mockResolvedValue('denied');

    const got = await notificationPermission();

    expect(invoke).toHaveBeenCalledWith('notification_permission');
    expect(got).toBe('denied');
  });

  it('openNotificationSettings が引数なしで invoke する', async () => {
    await openNotificationSettings();
    expect(invoke).toHaveBeenCalledWith('open_notification_settings');
  });

  it('worktreeStatus が sessionId を camelCase で渡し、戻り値をそのまま返す', async () => {
    invoke.mockResolvedValue({ dirty: true, entries: ['?? new.txt'] });

    const got = await worktreeStatus('s1');

    expect(invoke).toHaveBeenCalledWith('worktree_status', { sessionId: 's1' });
    expect(got).toEqual({ dirty: true, entries: ['?? new.txt'] });
  });

  it('cleanupWorktree が sessionId と force を camelCase で渡す', async () => {
    await cleanupWorktree('s1', true);
    expect(invoke).toHaveBeenCalledWith('cleanup_worktree', { sessionId: 's1', force: true });
  });

  it('cleanupWorktree は force が false のときもそのまま渡す（ハードコード禁止）', async () => {
    await cleanupWorktree('s1', false);
    expect(invoke).toHaveBeenCalledWith('cleanup_worktree', { sessionId: 's1', force: false });
  });
});
