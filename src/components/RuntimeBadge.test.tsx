import { Profiler, type ProfilerOnRenderCallback } from 'react';
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { RuntimeBadge, RuntimeBadgeView } from './RuntimeBadge';
import { useAppStore } from '../store';
import { KanbanCard } from '../views/KanbanView/KanbanCard';
import type { RuntimeState, Session, SessionStatePayload } from '../types/model';

const emit = (p: SessionStatePayload) => act(() => useAppStore.getState().applyStateEvent(p));

// 契約 §33.5（グリフとラベル）× §53.4（色トークン）の正典。
// 唯一の非自明点は waiting_input → --state-waiting（トークン名に _input が付かない）。
const CANON: Array<[RuntimeState, string, string]> = [
  ['running', '実行中', '--state-running'],
  ['waiting_input', '入力待ち', '--state-waiting'],
  ['idle', 'アイドル', '--state-idle'],
  ['exited', '終了', '--state-exited'],
  ['interrupted', '中断', '--state-interrupted'],
  ['error', 'エラー', '--state-error'],
];

// autoCleanup は vitest の globals が無効なので登録されない。明示的に片付ける
afterEach(cleanup);

beforeEach(() => {
  useAppStore.setState({ runtimeStates: {}, runtimeReasons: {}, runtimeErrors: {} });
});

describe('RuntimeBadgeView（純粋な描画。store に触らない）', () => {
  it('契約 §33.5 の 6 状態すべてをドットとラベルで描く', () => {
    for (const [state, label] of CANON) {
      cleanup();
      render(<RuntimeBadgeView state={state} />);
      const el = screen.getByRole('img');
      expect(el).toHaveAttribute('data-runtime-state', state);
      expect(el).toHaveAccessibleName(label);
      // 色だけで状態を示さない（デザインシステム）。ドット + テキストラベルの 2 要素
      expect(el).toHaveTextContent(label);
      expect(el.querySelector('.runtime-badge__dot')).not.toBeNull();
      expect(el.querySelector('.runtime-badge__label')).toHaveTextContent(label);
    }
  });

  it('reason があればツールチップに添える', () => {
    render(<RuntimeBadgeView state="waiting_input" reason="hook_notification" />);
    expect(screen.getByRole('img')).toHaveAttribute('title', '入力待ち (hook_notification)');
  });

  it('reason が無ければツールチップはラベルだけ', () => {
    render(<RuntimeBadgeView state="running" />);
    expect(screen.getByRole('img')).toHaveAttribute('title', '実行中');
  });
});

describe('RuntimeBadge の色（契約 §53.4: 正典は --state-* トークン）', () => {
  // 実装側で色を決めない。唯一の非自明点は waiting_input → --state-waiting で、
  // `--state-${state}` を機械的に組み立てるとここだけ外れる（トークンが存在せず黒く出る）
  it.each(CANON)('%s（%s）の色は var(%s)', (state, _label, token) => {
    render(<RuntimeBadgeView state={state} />);
    // ドットもラベルもここから currentColor で受ける（RuntimeBadge.css）
    expect(screen.getByRole('img')).toHaveStyle({ color: `var(${token})` });
  });
});

describe('RuntimeBadge（runtimeStates の唯一の購読者）', () => {
  // 契約 §33.3 Q1: 状態が未知のセッションには何も描かない（?? 'idle' は禁止）
  it('未知のセッションには何も描かない', () => {
    const { container } = render(<RuntimeBadge sessionId="s1" />);
    expect(screen.queryByRole('img')).toBeNull();
    expect(container).toBeEmptyDOMElement();
  });

  it('session://state を受けると 6 状態それぞれを描き替える', () => {
    render(<RuntimeBadge sessionId="s1" />);
    for (const [state, label] of CANON) {
      emit({ session_id: 's1', runtime_state: state, reason: 'spawned' });
      const el = screen.getByRole('img');
      expect(el).toHaveAttribute('data-runtime-state', state);
      expect(el).toHaveAccessibleName(label);
    }
  });

  it('reason をツールチップに出す', () => {
    render(<RuntimeBadge sessionId="s1" />);
    emit({ session_id: 's1', runtime_state: 'waiting_input', reason: 'hook_notification' });
    expect(screen.getByRole('img')).toHaveAttribute('title', '入力待ち (hook_notification)');
  });

  it('他セッションのイベントでは描画が変わらない', () => {
    render(<RuntimeBadge sessionId="s1" />);
    emit({ session_id: 's1', runtime_state: 'idle', reason: 'hook_stop' });
    emit({ session_id: 'other', runtime_state: 'running', reason: 'spawned' });
    expect(screen.getByRole('img')).toHaveAttribute('data-runtime-state', 'idle');
  });

  // プリミティブなセレクタ 2 本で読んでいる証拠。runtimeStates 全体を select すると
  // 無関係なセッションの遷移でも新しいオブジェクトが返り、ここで再レンダリングされる
  it('無関係なセッションの遷移では再レンダリングしない', () => {
    let renders = 0;
    const onRender: ProfilerOnRenderCallback = () => {
      renders += 1;
    };
    render(
      <Profiler id="badge" onRender={onRender}>
        <RuntimeBadge sessionId="s1" />
      </Profiler>,
    );
    expect(renders).toBe(1);

    emit({ session_id: 'other', runtime_state: 'running', reason: 'spawned' });

    expect(renders).toBe(1);
  });
});

function session(id: string): Session {
  return {
    id,
    project_id: 'p1',
    title: 'fix-login',
    description: '',
    kanban_status: 'in_progress',
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
    archived_at: null,
    created_at: 0,
    updated_at: 0,
  };
}

// 設計書 §5.3「列がバタつかない」= バッジの変化がカード全体の再レンダリングに波及しない。
// ローカル定義のダミーカードでは KanbanCard が runtimeStates を受け取っていても緑に
// なってしまうため、実物の KanbanCard を render する（契約 §38.3 の 3 点目）。
describe('KanbanCard の中の RuntimeBadge（契約 §25.5 の不変条件）', () => {
  /** KanbanCard の関数本体が再実行されたかを、描画中に読まれる session.title で数える。 */
  function probeSession(): { session: Session; titleReads: () => number } {
    const base = session('s1');
    let reads = 0;
    const probe: Session = Object.defineProperty({ ...base }, 'title', {
      get() {
        reads += 1;
        return base.title;
      },
    });
    return { session: probe, titleReads: () => reads };
  }

  it('バッジが変化してもカードは再レンダリングされない', () => {
    const { session: probe, titleReads } = probeSession();
    render(<KanbanCard session={probe} />);
    expect(titleReads()).toBe(1);

    emit({ session_id: 's1', runtime_state: 'running', reason: 'spawned' });
    emit({ session_id: 's1', runtime_state: 'waiting_input', reason: 'hook_notification' });

    expect(screen.getByRole('img')).toHaveAttribute('data-runtime-state', 'waiting_input');
    expect(titleReads()).toBe(1);
  });

  it('エラーが入ってもカードは再レンダリングされない', () => {
    const { session: probe, titleReads } = probeSession();
    render(<KanbanCard session={probe} />);
    expect(titleReads()).toBe(1);

    emit({ session_id: 's1', runtime_state: 'error', reason: 'spawn_failed' });
    act(() => useAppStore.getState().setRuntimeError('s1', 'boom'));

    expect(screen.getByText('boom')).toHaveClass('kanban-card__error');
    expect(titleReads()).toBe(1);
  });
});
