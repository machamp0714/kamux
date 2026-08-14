import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { KanbanCardResume } from './KanbanCardResume';
import { useAppStore } from '../../store';
import type { Session } from '../../types/model';

afterEach(cleanup);

function session(over: Partial<Session>): Session {
  return {
    id: 's1',
    project_id: 'p1',
    title: 't',
    description: '',
    kanban_status: 'backlog',
    sort_order: 1,
    mode: 'in_place',
    branch: null,
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
    ...over,
  };
}

const resumeSession = vi.fn(async () => undefined);
const retryResumeAsFresh = vi.fn(async () => undefined);

beforeEach(() => {
  resumeSession.mockClear();
  retryResumeAsFresh.mockClear();
  useAppStore.setState({
    runtimeStates: {},
    sessions: {},
    resumeFailedSessionIds: [],
    resumeSession,
    retryResumeAsFresh,
  });
});

describe('KanbanCardResume（第1部 §4.4）', () => {
  it('interrupted / exited 以外は何も描かない', () => {
    useAppStore.setState({
      runtimeStates: { s1: 'running' },
      sessions: { s1: session({}) },
    });
    const { container } = render(<KanbanCardResume sessionId="s1" />);
    expect(container).toBeEmptyDOMElement();
  });

  it.each(['interrupted', 'exited'] as const)(
    '%s のときは resumeAffordance のラベルでボタンを描く',
    (state) => {
      useAppStore.setState({
        runtimeStates: { s1: state },
        sessions: { s1: session({ claude_session_id: 'cs1' }) },
      });
      render(<KanbanCardResume sessionId="s1" />);
      expect(screen.getByRole('button', { name: '会話を再開' })).toBeInTheDocument();
    },
  );

  it('ボタンを押すと store.resumeSession が呼ばれる（retryResumeAsFresh は呼ばれない）', () => {
    useAppStore.setState({
      runtimeStates: { s1: 'interrupted' },
      sessions: { s1: session({ claude_session_id: 'cs1' }) },
    });
    render(<KanbanCardResume sessionId="s1" />);

    fireEvent.click(screen.getByRole('button', { name: '会話を再開' }));

    expect(resumeSession).toHaveBeenCalledWith('s1');
    expect(retryResumeAsFresh).not.toHaveBeenCalled();
  });

  it('resumeFailedSessionIds に載っていれば「新しい会話として開始」に切り替わる', () => {
    useAppStore.setState({
      runtimeStates: { s1: 'exited' },
      sessions: { s1: session({ claude_session_id: 'cs1' }) },
      resumeFailedSessionIds: ['s1'],
    });
    render(<KanbanCardResume sessionId="s1" />);

    expect(screen.getByRole('button', { name: '新しい会話として開始' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '会話を再開' })).not.toBeInTheDocument();
  });

  it('失敗リストのボタンを押すと store.retryResumeAsFresh が呼ばれる（resumeSession は呼ばれない）', () => {
    useAppStore.setState({
      runtimeStates: { s1: 'exited' },
      sessions: { s1: session({ claude_session_id: 'cs1' }) },
      resumeFailedSessionIds: ['s1'],
    });
    render(<KanbanCardResume sessionId="s1" />);

    fireEvent.click(screen.getByRole('button', { name: '新しい会話として開始' }));

    expect(retryResumeAsFresh).toHaveBeenCalledWith('s1');
    expect(resumeSession).not.toHaveBeenCalled();
  });

  it('ラベルは resumeAffordance の返り値に追随する（codex は会話を復元しない）', () => {
    useAppStore.setState({
      runtimeStates: { s1: 'interrupted' },
      sessions: { s1: session({ cli_kind: 'codex', claude_session_id: null }) },
    });
    render(<KanbanCardResume sessionId="s1" />);
    expect(screen.getByRole('button', { name: 'プロセスを再起動' })).toBeInTheDocument();
  });

  it('クリックはカードの onClick へバブリングしない（stopPropagation）', () => {
    useAppStore.setState({
      runtimeStates: { s1: 'interrupted' },
      sessions: { s1: session({ claude_session_id: 'cs1' }) },
    });
    const onCardClick = vi.fn();
    render(
      <div onClick={onCardClick}>
        <KanbanCardResume sessionId="s1" />
      </div>,
    );

    fireEvent.click(screen.getByRole('button', { name: '会話を再開' }));

    expect(onCardClick).not.toHaveBeenCalled();
  });
});
