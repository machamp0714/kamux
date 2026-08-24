import { describe, expect, it } from 'vitest';
import type { Session } from '../types/model';
import { isCleanupSuggested } from './cleanup';

const s = (over: Partial<Session>): Session => ({
  id: 's1',
  project_id: 'p1',
  title: 't',
  description: '',
  kanban_status: 'done',
  sort_order: 1,
  mode: 'worktree',
  branch: 'session/t',
  worktree_path: '/repo/a/.worktrees/session-t',
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
  ...over,
});

describe('isCleanupSuggested', () => {
  it('Done 列の worktree セッションでは提案する', () => {
    expect(isCleanupSuggested(s({}))).toBe(true);
  });

  it('アーカイブ済みなら列に関わらず提案する', () => {
    expect(
      isCleanupSuggested(s({ kanban_status: 'in_progress', archived_at: 1754006400000 })),
    ).toBe(true);
  });

  it('作業中（Done でもアーカイブでもない）は提案しない', () => {
    expect(isCleanupSuggested(s({ kanban_status: 'in_progress' }))).toBe(false);
  });

  it('in_place セッションでは提案しない', () => {
    expect(isCleanupSuggested(s({ mode: 'in_place', worktree_path: null }))).toBe(false);
  });

  it('掃除済み（worktree_path が null）なら提案しない', () => {
    expect(isCleanupSuggested(s({ worktree_path: null }))).toBe(false);
  });
});
