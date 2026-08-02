import type { CliKind, RuntimeState } from '../../types/model';

/**
 * 設計書 §6.2 / 契約 §2 の runtime バッジ。
 * runtime_state が表すのは `SurfaceKind::Agent` の PTY だけで、
 * `SurfaceKind::Editor`（nvim、M3-1）の起動・終了はここに現れない（契約 §2）。
 */
// グリフとラベルの正典は契約 §33.5。**6 値すべてを埋めること**
// （Record<RuntimeState, string> なので 5 値では型エラーになる）。
export const RUNTIME_BADGE: Record<RuntimeState, string> = {
  running: '🟢',
  waiting_input: '🟡',
  idle: '⚪',
  exited: '⛔',
  interrupted: '⏸',
  error: '❌',
};

export const RUNTIME_BADGE_LABEL: Record<RuntimeState, string> = {
  running: '実行中',
  waiting_input: '入力待ち',
  idle: 'アイドル',
  exited: '終了',
  interrupted: '中断',
  error: 'エラー',
};

export const CLI_ICON: Record<CliKind, string> = {
  claude: '✳',
  codex: '◇',
  shell: '❯',
  custom: '⚙',
};

export function runtimeBadge(
  runtimeStates: Record<string, RuntimeState>,
  sessionId: string,
): { glyph: string; label: string } | null {
  const state = runtimeStates[sessionId];
  // 値が無い = 一度も起動していない or 最初の session://state 到着前。
  // どちらもバッジを描かない（契約 §33.3 Q1 / §34.7）。
  // session.last_runtime_state へはフォールバックしない（第1部 判断 6）—— DB の
  // running / waiting_input を interrupted へ正規化するのは M2-1 の
  // normalize_on_startup() であり、M1-2 の時点では走っていないため。
  if (state === undefined) return null;
  return { glyph: RUNTIME_BADGE[state], label: RUNTIME_BADGE_LABEL[state] };
}
