import type { ResumePlan, Session } from '../types/model';

export interface ResumeAffordance {
  plan: ResumePlan;
  label: string;
  /** ボタン下に出す補足。null なら出さない */
  note: string | null;
  /** 会話が意図せず途切れることをユーザーに警告すべきか */
  warn: boolean;
}

/**
 * 再開ボタンの表示を決める。docs/superpowers/plans/2026-08-01-kamux/M2-4-resume.md 第1部 §3 の分岐表を
 * Rust の resume_plan()（src-tauri/src/session/cli_args.rs）と同じ順序で写経したもの。
 * 片方を変えたら必ず両方変える。
 */
export function resumeAffordance(session: Session): ResumeAffordance {
  if (session.cli_kind !== 'claude') {
    return {
      plan: { kind: 'fresh_start', reason: 'no_conversation_restore' },
      label: 'プロセスを再起動',
      note: '会話は復元されません',
      warn: false,
    };
  }

  if (session.claude_session_id !== null) {
    return {
      plan: {
        kind: 'claude_resume',
        claude_session_id: session.claude_session_id,
      },
      label: '会話を再開',
      note: null,
      warn: false,
    };
  }

  if (session.mode === 'worktree') {
    return {
      plan: { kind: 'claude_continue' },
      label: '会話を再開',
      note: 'この作業ツリーの最新の会話に接続します',
      warn: false,
    };
  }

  // in_place は同一 cwd に複数会話がありうるため --continue を使わない（§4.1）
  return {
    plan: { kind: 'fresh_start', reason: 'ambiguous_in_place_conversation' },
    label: '新しい会話で開始',
    note: 'この作業ツリーの会話を特定できないため、新しい会話として開始します',
    warn: true,
  };
}
