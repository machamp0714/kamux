import type { Session, WorktreeStatus } from '../types/model';

export interface CleanupDialogState {
  sessionId: string;
  /** null = 取得中 or 取得失敗 */
  status: WorktreeStatus | null;
  /** git / IPC の生メッセージ。加工しない（契約 §6） */
  error: string | null;
  busy: boolean;
}

/**
 * worktree 掃除を提案してよいセッションかどうか。
 * 提案は押し付けない: この関数が true のときにカード上へ控えめなボタンを出すだけで、
 * モーダルを自動で開いたりはしない。
 */
export function isCleanupSuggested(s: Session): boolean {
  if (s.mode !== 'worktree') return false;
  if (s.worktree_path === null) return false;
  return s.kanban_status === 'done' || s.archived_at !== null;
}
