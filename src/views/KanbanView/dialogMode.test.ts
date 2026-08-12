import { describe, expect, it } from 'vitest';
import type { Session } from '../../types/model';
import { resolveDialogMode } from './dialogMode';

function makeSession(overrides: Partial<Session> & { id: string }): Session {
  return {
    project_id: 'p1',
    title: 'Fix Login Bug',
    description: 'ログインが落ちる',
    kanban_status: 'backlog',
    sort_order: 1,
    mode: 'worktree',
    branch: 'session/fix-login-bug',
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
    ...overrides,
  };
}

describe('resolveDialogMode', () => {
  it('create_session モーダルでは editingSession を無視して create を返す', () => {
    expect(resolveDialogMode({ kind: 'create_session' }, null)).toEqual({ kind: 'create' });
    expect(resolveDialogMode({ kind: 'create_session' }, makeSession({ id: 's1' }))).toEqual({
      kind: 'create',
    });
  });

  it('edit_session モーダルで対象セッションが見つかれば edit を返す', () => {
    const session = makeSession({ id: 's1' });
    expect(resolveDialogMode({ kind: 'edit_session', sessionId: 's1' }, session)).toEqual({
      kind: 'edit',
      session,
    });
  });

  it('edit_session モーダルで対象セッションがストアに無ければ lost を返す（作成モードへフォールバックしない）', () => {
    expect(resolveDialogMode({ kind: 'edit_session', sessionId: 's1' }, null)).toEqual({
      kind: 'lost',
    });
  });
});
