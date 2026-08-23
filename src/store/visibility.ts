import type { AppStore } from './index';

export type VisibilityInput = Pick<
  AppStore,
  'view' | 'layout' | 'paneAssignment' | 'focusedSessionId'
>;

/**
 * 「今ユーザーの目に入っているセッション」を返す。
 *
 * Rust 側の通知抑制判定（notify::policy::is_session_visible）に渡す唯一の材料。
 * ターミナル画面以外は agent の出力が見えていないので空配列になる。
 */
export function visibleSessionIds(s: VisibilityInput): string[] {
  if (s.view !== 'terminal') return [];
  // 契約 §28.2: ここは `=== 'single'` のままで正しい。split2 / split2-v の
  // どちらでも両ペインが見えているため、isSplit() に書き換えないこと
  if (s.layout === 'single') {
    return s.focusedSessionId ? [s.focusedSessionId] : [];
  }
  const assigned = s.paneAssignment.filter((id): id is string => id !== null);
  return [...new Set(assigned)];
}
