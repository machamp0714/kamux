import { Profiler, type ProfilerOnRenderCallback } from 'react';
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { RuntimeBadge, RuntimeBadgeView, badgeTooltip, isEstimated } from './RuntimeBadge';
import { useAppStore } from '../store';
import { KanbanCard } from '../views/KanbanView/KanbanCard';
import type { RuntimeState, Session, SessionStatePayload, StateReason } from '../types/model';

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
    // badgeTooltip は権威ある reason では LABEL[state] だけを返す（括弧付きの reason は出さない）
    expect(screen.getByRole('img')).toHaveAttribute('title', '入力待ち');
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
    // badgeTooltip は権威ある reason では LABEL[state] だけを返す（括弧付きの reason は出さない）
    expect(screen.getByRole('img')).toHaveAttribute('title', '入力待ち');
  });

  it('他セッションのイベントでは描画が変わらない', () => {
    render(<RuntimeBadge sessionId="s1" />);
    emit({ session_id: 's1', runtime_state: 'idle', reason: 'hook_stop' });
    emit({ session_id: 'other', runtime_state: 'running', reason: 'spawned' });
    expect(screen.getByRole('img')).toHaveAttribute('data-runtime-state', 'idle');
  });

  // M3-3 修正ラウンド 1（レビュー I-1）: runtimeReasons[sessionId] セレクタが
  // ヒューリスティック由来の reason を実際に RuntimeBadgeView まで届けているかは
  // 純関数（isEstimated / badgeTooltip / RuntimeBadgeView）のテストだけでは守れない
  // ——ストア→セレクタ→ビューの継ぎ目が無検査だと、runtimeReasons を誤ったキーで
  // 読んでも気づけない（変異検証で確認）。`~` 前置は toHaveTextContent の部分一致
  // ではなく textContent の完全一致で確かめる（`~` が落ちても "アイドル" は
  // "~アイドル" の部分文字列ではないので拾えるが、念のため完全一致にしておく）。
  it('推定 reason（silence_timeout）を受けると中空ドット・~前置・推定ツールチップに切り替わる', () => {
    render(<RuntimeBadge sessionId="s1" />);
    emit({ session_id: 's1', runtime_state: 'idle', reason: 'silence_timeout' });
    const el = screen.getByRole('img');
    expect(el.querySelector('.runtime-badge__label')?.textContent).toBe('~アイドル');
    expect(el.className).toContain('runtime-badge--estimated');
    expect(el.getAttribute('title')).toContain('推定');
  });

  // bel_detected は silence_timeout とは別の reason 値なので、同じストア経路
  // （runtimeReasons[sessionId] → isEstimated → badgeTooltip）を通ることを
  // 別の入力で確かめる。ツールチップの分岐文言（「ベル文字を検知」）が
  // silence_timeout 側と異なるため、tooltip 組み立ての取り違えも拾える。
  it('推定 reason（bel_detected）を受けても同じ経路で中空ドット・~前置に切り替わる', () => {
    render(<RuntimeBadge sessionId="s1" />);
    emit({ session_id: 's1', runtime_state: 'waiting_input', reason: 'bel_detected' });
    const el = screen.getByRole('img');
    expect(el.querySelector('.runtime-badge__label')?.textContent).toBe('~入力待ち');
    expect(el.className).toContain('runtime-badge--estimated');
    expect(el.getAttribute('title')).toContain('ベル文字');
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
    heuristics_enabled: true,
    silence_timeout_secs: 30,
    is_scratch: false,
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

// M3-3 Task 16: 汎用 CLI 向けヒューリスティック（BEL 検知 / 沈黙判定）由来の
// 「推定」表示。契約 §76.2 によりグリフの描画・検証は行わない —— 実装が持つのは
// ドット + ラベル（M2-1）のみで、推定表示はそこへ中空ドット・`~` 前置ラベル
// （確定仕様は .claude/skills/kamux-design-system/components.md「実行状態バッジ」
// 節）と、ツールチップ（組み立て正典は契約 §33.5 末尾 / badgeTooltip）を足す形。
describe('isEstimated', () => {
  // StateReason は 13 値（src/types/model.ts）。配列リテラルでは新しい値が増えても
  // 更新が強制されないため、Record<StateReason, boolean> で網羅する
  // （M3-3 Task 15 の sessionSlice.heuristics.test.ts と同じ形）。
  const EXPECTED: Record<StateReason, boolean> = {
    spawned: false,
    hook_notification: false,
    hook_stop: false,
    pty_exited: false,
    startup_normalize: false,
    bel_detected: true,
    silence_timeout: true,
    user_stopped: false,
    output_activity: false,
    user_input: false,
    hook_permission: false,
    resume_failed: false,
    spawn_failed: false,
  };

  it('flags only the two heuristic-derived reasons as estimated', () => {
    for (const reason of Object.keys(EXPECTED) as StateReason[]) {
      expect(isEstimated(reason)).toBe(EXPECTED[reason]);
    }
  });

  it('treats a missing reason as certain', () => {
    expect(isEstimated(undefined)).toBe(false);
  });
});

describe('badgeTooltip', () => {
  it('names the state plainly for authoritative reasons', () => {
    expect(badgeTooltip('idle', 'hook_stop')).toBe('アイドル');
    expect(badgeTooltip('waiting_input', 'hook_notification')).toBe('入力待ち');
  });

  it('marks silence-derived states as estimated and explains why', () => {
    const tip = badgeTooltip('idle', 'silence_timeout');
    expect(tip).toContain('アイドル');
    expect(tip).toContain('推定');
    expect(tip).toContain('出力が一定時間停止');
    expect(tip).toContain('誤検知');
  });

  it('marks bel-derived states as estimated and explains why', () => {
    const tip = badgeTooltip('waiting_input', 'bel_detected');
    expect(tip).toContain('入力待ち');
    expect(tip).toContain('推定');
    expect(tip).toContain('ベル文字');
    expect(tip).toContain('誤検知');
  });
});

describe('RuntimeBadgeView の推定表示（components.md「実行状態バッジ」節 / 契約 §76.1: 中空ドット + `~` 前置ラベル）', () => {
  // 契約 §76.2: 「グリフを網羅するテスト」の代わりに、権威 / 推定の両方で
  // 6 状態のラベルが正しく描かれることを網羅する。ラベル文字列は CANON
  // （このテストファイル内で固定した表。production の RUNTIME_BADGE_LABEL
  // からは導出しない）に `~` を前置するかどうかだけを完全一致で確かめる。
  it('renders the correct label for every state under both authoritative and estimated reasons', () => {
    for (const [state, label] of CANON) {
      const { unmount: unmountAuthoritative } = render(
        <RuntimeBadgeView state={state} reason="spawned" />,
      );
      expect(screen.getByRole('img').querySelector('.runtime-badge__label')?.textContent).toBe(
        label,
      );
      expect(screen.getByRole('img').getAttribute('data-estimated')).toBe('false');
      unmountAuthoritative();

      const { unmount: unmountEstimated } = render(
        <RuntimeBadgeView state={state} reason="silence_timeout" />,
      );
      expect(screen.getByRole('img').querySelector('.runtime-badge__label')?.textContent).toBe(
        `~${label}`,
      );
      expect(screen.getByRole('img').getAttribute('data-estimated')).toBe('true');
      unmountEstimated();
    }
  });

  it('flags estimated states with data-estimated="true"', () => {
    render(<RuntimeBadgeView state="idle" reason="silence_timeout" />);
    expect(screen.getByRole('img').getAttribute('data-estimated')).toBe('true');
  });

  it('does not flag authoritative states', () => {
    render(<RuntimeBadgeView state="idle" reason="hook_stop" />);
    expect(screen.getByRole('img').getAttribute('data-estimated')).toBe('false');
  });

  it('exposes the tooltip through title and aria-label', () => {
    render(<RuntimeBadgeView state="waiting_input" reason="bel_detected" />);
    const el = screen.getByRole('img');
    expect(el.getAttribute('title')).toContain('推定');
    expect(el.getAttribute('aria-label')).toContain('推定');
  });

  it('applies the estimated modifier class used by the hollow dot', () => {
    render(<RuntimeBadgeView state="idle" reason="silence_timeout" />);
    expect(screen.getByRole('img').className).toContain('runtime-badge--estimated');
  });

  it('does not apply the estimated modifier class for authoritative reasons', () => {
    render(<RuntimeBadgeView state="idle" reason="hook_stop" />);
    expect(screen.getByRole('img').className).not.toContain('runtime-badge--estimated');
  });
});
