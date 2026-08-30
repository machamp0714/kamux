import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import {
  ackPty,
  cleanupWorktree,
  createProject,
  createScratchSession,
  createSession,
  deleteProject,
  getHooksDiagnostics,
  listProjects,
  listSessions,
  moveSession,
  notificationPermission,
  openNotificationSettings,
  reportFrontendReady,
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
import type { Session } from '../types/model';

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

  // 契約 §7.1 が逐語で `delete_project` を指定している。コマンド名を固定する。
  it('deleteProject が契約どおりのコマンド名と id 引数で invoke する', async () => {
    await deleteProject('p1');
    expect(invoke).toHaveBeenCalledWith('delete_project', { id: 'p1' });
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

  // 契約 §29.3: create_scratch_session(project_id, cwd) を camelCase 引数で invoke する。
  it('createScratchSession が projectId と cwd を camelCase で渡し、戻り値の Session をそのまま返す', async () => {
    const created: Session = {
      id: 's-scratch',
      project_id: 'p1',
      title: 'scratch',
      description: '',
      kanban_status: 'backlog',
      sort_order: 1,
      mode: 'in_place',
      branch: null,
      worktree_path: null,
      cli_kind: 'shell',
      cli_command: null,
      claude_session_id: null,
      last_runtime_state: 'running',
      last_runtime_error: null,
      first_started_at: 1,
      heuristics_enabled: false,
      silence_timeout_secs: 30,
      is_scratch: true,
      archived_at: null,
      created_at: 1,
      updated_at: 1,
    };
    invoke.mockResolvedValue(created);

    const got = await createScratchSession('p1', '/Users/x/repo/kamux');

    expect(invoke).toHaveBeenCalledWith('create_scratch_session', {
      projectId: 'p1',
      cwd: '/Users/x/repo/kamux',
    });
    expect(got).toEqual(created);
  });

  // 契約 §29.3: cwd が None なら project.repo_path。呼び出し側が明示的に null を渡すことで
  // その分岐に入る（省略可能にすると渡し忘れと区別できなくなる）。
  it('createScratchSession は cwd に null を渡せる', async () => {
    await createScratchSession('p1', null);
    expect(invoke).toHaveBeenCalledWith('create_scratch_session', {
      projectId: 'p1',
      cwd: null,
    });
  });

  // 契約 §29.3: cwd は省略可能にしない（型レベル）。渡し忘れと意図的な null が
  // 区別できなくなるため、TS の引数省略そのものをコンパイルエラーにする。
  // cwd が optional になると @ts-expect-error が「使われていない」扱いになり
  // TS2578 で tsc が落ちる —— これが唯一の観測点（実行時には JS は arity を強制しない）。
  it('createScratchSession は cwd を省略するとコンパイルエラーになる', () => {
    // @ts-expect-error cwd は必須引数（契約 §29.3）
    void createScratchSession('p1');
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

  // 契約 §7.1 が `report_frontend_ready() -> AppResult<()>` を逐語で確定させている
  // （コメントは「契約 §0 の『起動 1 秒未満』の測定に必須」と動機を添える）。
  // scripts/measure-perf.sh（Task 14）の grep 対象はこの文字列に依存するため、
  // 引数なしで正確な名前を渡していることをここで固定する。
  it('reportFrontendReady が引数なしで report_frontend_ready を invoke する', async () => {
    await reportFrontendReady();
    expect(invoke).toHaveBeenCalledWith('report_frontend_ready');
  });
});
