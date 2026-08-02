import type { ModalState } from '../../store/uiSlice';
import type { Session } from '../../types/model';

export type DialogMode = { kind: 'create' } | { kind: 'edit'; session: Session } | { kind: 'lost' };

/**
 * modal の意図（新規作成 / 編集）と、ストアから読んだ編集対象セッションの現況から
 * ダイアログの実際の表示モードを決める。
 *
 * 編集モードで開いている間に対象セッションがストアから消えた場合（アーカイブ等）、
 * 作成モードへフォールバックしない。フォールバックすると、編集中の title/description の
 * 入力値を保持したまま addSession が呼ばれ「編集のつもりが複製作成」になってしまう
 * （task-15-report 修正ラウンド 1）。
 */
export function resolveDialogMode(modal: ModalState, editingSession: Session | null): DialogMode {
  if (modal.kind === 'create_session') return { kind: 'create' };
  return editingSession === null ? { kind: 'lost' } : { kind: 'edit', session: editingSession };
}
