import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { InterruptedOverlay } from './InterruptedOverlay';
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
    mode: 'worktree',
    branch: 'feat/x',
    worktree_path: '/tmp/x',
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
    sessions: {},
    resumeFailedSessionIds: [],
    resumeSession,
    retryResumeAsFresh,
  });
});

describe('InterruptedOverlay（第1部 §4.4。まだどこにもマウントされていない）', () => {
  it('session が無ければ何も描かない', () => {
    const { container } = render(<InterruptedOverlay sessionId="nope" />);
    expect(container).toBeEmptyDOMElement();
  });

  it('通常時は resumeAffordance のラベルのボタンを描き、押すと store.resumeSession を呼ぶ', () => {
    useAppStore.setState({
      sessions: { s1: session({ mode: 'worktree', claude_session_id: null }) },
    });
    render(<InterruptedOverlay sessionId="s1" />);

    const button = screen.getByRole('button', { name: '会話を再開' });
    fireEvent.click(button);

    expect(resumeSession).toHaveBeenCalledWith('s1');
    expect(retryResumeAsFresh).not.toHaveBeenCalled();
  });

  it('note が warn のときは警告マーク付きで表示する', () => {
    useAppStore.setState({
      sessions: { s1: session({ mode: 'in_place', claude_session_id: null }) },
    });
    render(<InterruptedOverlay sessionId="s1" />);

    expect(
      screen.getByText('この作業ツリーの会話を特定できないため、新しい会話として開始します', {
        exact: false,
      }),
    ).toHaveClass('interrupted-overlay__warn');
  });

  it('失敗リストに載っていれば「新しい会話として開始」に切り替わり、押すと retryResumeAsFresh を呼ぶ', () => {
    useAppStore.setState({
      sessions: { s1: session({ claude_session_id: 'cs1' }) },
      resumeFailedSessionIds: ['s1'],
    });
    render(<InterruptedOverlay sessionId="s1" />);

    expect(screen.queryByRole('button', { name: '会話を再開' })).not.toBeInTheDocument();
    const button = screen.getByRole('button', { name: '新しい会話として開始' });
    fireEvent.click(button);

    expect(retryResumeAsFresh).toHaveBeenCalledWith('s1');
    expect(resumeSession).not.toHaveBeenCalled();
  });
});
