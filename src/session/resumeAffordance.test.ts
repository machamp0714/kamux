import { describe, expect, it } from 'vitest';
import { resumeAffordance } from './resumeAffordance';
import type { CliKind, Session, SessionMode } from '../types/model';

const ID = '550e8400-e29b-41d4-a716-446655440000';

function session(cli_kind: CliKind, claude_session_id: string | null, mode: SessionMode): Session {
  return {
    id: '11111111-1111-4111-8111-111111111111',
    project_id: '22222222-2222-4222-8222-222222222222',
    title: 'fix login',
    description: '',
    kanban_status: 'in_progress',
    sort_order: 1,
    mode,
    branch: mode === 'worktree' ? 'session/fix-login' : null,
    worktree_path: mode === 'worktree' ? '/repo/.worktrees/session-fix-login' : null,
    cli_kind,
    cli_command: cli_kind === 'custom' ? 'my-agent --flag' : null,
    claude_session_id,
    last_runtime_state: 'interrupted',
    last_runtime_error: null,
    first_started_at: 1,
    heuristics_enabled: true,
    silence_timeout_secs: 30,
    is_scratch: false,
    archived_at: null,
    created_at: 0,
    updated_at: 0,
  };
}

interface Row {
  cli_kind: CliKind;
  mode: SessionMode;
  claude_session_id: string | null;
}

// 分岐表（計画第1部 §3）の 16 行を cli_kind / mode / claude_session_id の直積として
// 配列リテラルから生成する。ここが 4 種類のいずれか 1 つでも欠けると ALL_ROWS の
// 長さが 16 未満になり、下の「16 行であること」のテストが赤くなる。
const CLI_KINDS: readonly CliKind[] = ['claude', 'codex', 'shell', 'custom'];
const MODES: readonly SessionMode[] = ['worktree', 'in_place'];
const CLAUDE_SESSION_IDS: readonly (string | null)[] = [ID, null];

const ALL_ROWS: Row[] = CLI_KINDS.flatMap((cli_kind) =>
  MODES.flatMap((mode) =>
    CLAUDE_SESSION_IDS.map((claude_session_id) => ({ cli_kind, mode, claude_session_id })),
  ),
);

describe('resumeAffordance', () => {
  it('分岐表の直積が 16 行であること（フィクスチャ自身の網羅性の担保）', () => {
    expect(ALL_ROWS).toHaveLength(16);
  });

  it('行 1/2: ID があれば会話を再開する', () => {
    const rows = ALL_ROWS.filter((r) => r.cli_kind === 'claude' && r.claude_session_id !== null);
    expect(rows).toHaveLength(2);
    for (const row of rows) {
      const a = resumeAffordance(session(row.cli_kind, row.claude_session_id, row.mode));
      expect(a.plan).toEqual({ kind: 'claude_resume', claude_session_id: ID });
      expect(a.label).toBe('会話を再開');
      expect(a.note).toBeNull();
      expect(a.warn).toBe(false);
    }
  });

  it('行 3: worktree で ID が無ければ --continue 相当', () => {
    const rows = ALL_ROWS.filter(
      (r) => r.cli_kind === 'claude' && r.claude_session_id === null && r.mode === 'worktree',
    );
    expect(rows).toHaveLength(1);
    for (const row of rows) {
      const a = resumeAffordance(session(row.cli_kind, row.claude_session_id, row.mode));
      expect(a.plan).toEqual({ kind: 'claude_continue' });
      expect(a.label).toBe('会話を再開');
      expect(a.note).toBe('この作業ツリーの最新の会話に接続します');
      expect(a.warn).toBe(false);
    }
  });

  it('行 4: in_place で ID が無ければ新規会話 + 警告', () => {
    const rows = ALL_ROWS.filter(
      (r) => r.cli_kind === 'claude' && r.claude_session_id === null && r.mode === 'in_place',
    );
    expect(rows).toHaveLength(1);
    for (const row of rows) {
      const a = resumeAffordance(session(row.cli_kind, row.claude_session_id, row.mode));
      expect(a.plan).toEqual({
        kind: 'fresh_start',
        reason: 'ambiguous_in_place_conversation',
      });
      expect(a.label).toBe('新しい会話で開始');
      expect(a.note).toBe('この作業ツリーの会話を特定できないため、新しい会話として開始します');
      expect(a.warn).toBe(true);
    }
  });

  it('行 5-16: codex / shell / custom は会話復元なし', () => {
    const rows = ALL_ROWS.filter((r) => r.cli_kind !== 'claude');
    expect(rows).toHaveLength(12);
    for (const row of rows) {
      const a = resumeAffordance(session(row.cli_kind, row.claude_session_id, row.mode));
      expect(a.plan).toEqual({
        kind: 'fresh_start',
        reason: 'no_conversation_restore',
      });
      expect(a.label).toBe('プロセスを再起動');
      expect(a.note).toBe('会話は復元されません');
      expect(a.warn).toBe(false);
    }
  });
});
