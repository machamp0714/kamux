import { useState } from 'react';

import './DeleteProjectDialog.css';

export interface DeleteProjectDialogProps {
  /** 消す対象のプロジェクト名（契約 §130.4）。 */
  projectName: string;
  /**
   * 一緒に消えるセッション数。`sessions` は契約 §3 の ON DELETE CASCADE で消える。
   * **null = まだ数えられていない**（`list_sessions` の往復中 or 失敗）。
   * このとき件数を主張しないこと —— 0 と書くと、破壊操作の確認ダイアログが安心側に嘘をつく。
   */
  sessionCount: number | null;
  /**
   * そのうち現在稼働中の件数。数えるのは `DeleteProjectDialogContainer` で、
   * 判定は `CleanupWorktreeDialog.tsx` の `live` と同じ 2 値に揃えてある。
   * `undefined`（実行状態が未知）は数えない —— 知らないときに「動いています」とは言わない。
   * null = `sessionCount` と同じく、まだ数えられていない。
   */
  liveCount: number | null;
  /** 件数の取得が失敗したときの生メッセージ。加工しない（契約 §6）。 */
  error: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * プロジェクト削除の確認ダイアログ（純表示。ストアには触らない）。
 * ストアとの結線は `DeleteProjectDialogContainer` が持つ。
 * 契約 §130.3 によりマウント先は `ProjectBar`（管理面）であって `ProjectSwitcher` ではない。
 *
 * **`aria-modal` は宣言しない。** `CleanupWorktreeDialog.tsx` の doc と同じ判断に揃えた ——
 * `aria-modal` は「外側は無効化されている」と宣言するが、本実装は外側を無効化していない
 * （focus trap も `inert` も無い）。加えて `deleteProjectDialog` は `uiSlice` の `modal` とは
 * 別フィールドなので `handleKeymapKeyDown`（`useKeymap.ts`）が組み立てる `modalOpen` 判定に
 * 乗らず、Escape で閉じる手段も無い。宣言と実態が食い違うと、支援技術の利用者は外側の
 * 内容を隠されたまま慣習的な閉じ方も持たない。閉じる手段はキャンセルボタンとスクリムの 2 つ。
 */
export function DeleteProjectDialog({
  projectName,
  sessionCount,
  liveCount,
  error,
  onConfirm,
  onCancel,
}: DeleteProjectDialogProps) {
  // components.md「モーダル・ダイアログ」: 不可逆操作は明示的なチェックボックスを通す。
  // プロジェクト行とセッション行は削除すると復元できない（worktree とブランチは残る）。
  const [confirmed, setConfirmed] = useState(false);

  // 何が消えるのか分からないまま確定させない（`CleanupWorktreeDialog` の
  // `canDelete = status !== null && …` と同じ形）。取得に失敗したままなら確定できない。
  const canDelete = sessionCount !== null && confirmed;

  return (
    <div className="delete-project-dialog__backdrop" onMouseDown={onCancel}>
      <div
        className="delete-project-dialog"
        role="dialog"
        aria-label="プロジェクトを削除"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="delete-project-dialog__header">
          {/* components.md「破壊的な確認ダイアログ」: 左肩に 32×32 のアイコン枠 */}
          <span className="delete-project-dialog__icon" aria-hidden="true">
            ⚠
          </span>
          <h2 className="delete-project-dialog__title">プロジェクトを削除します</h2>
        </header>

        <div className="delete-project-dialog__body">
          <p className="delete-project-dialog__name">{projectName}</p>

          {/* components.md「破壊的な確認ダイアログ」: 危険の内訳は --bg-app + 1px solid
              --state-error のブロックへ入れる。本文は赤くしない */}
          <div className="delete-project-dialog__impact">
            {/* 知らないときに断定しない（契約 §38.3 論点 2 と同じ規律）。0 件とは書かない */}
            {sessionCount === null && error === null && (
              <p className="delete-project-dialog__loading">セッションを数えています…</p>
            )}
            {sessionCount !== null && (
              <p className="delete-project-dialog__count">
                セッション {sessionCount} 件が一緒に消えます
              </p>
            )}
            {liveCount !== null && liveCount > 0 && (
              <p className="delete-project-dialog__live">うち稼働中 {liveCount} 件</p>
            )}
            {/* 契約 §6: IPC の生メッセージを加工せず原文で出す */}
            {error !== null && <pre className="delete-project-dialog__error">{error}</pre>}
          </div>

          {/* 契約 §130.4: worktree は消さない。§13 が「git branch -D は決して実行しない」と
              定めているのと同じ性格の判断で、消す導線は 🧹（plan_cleanup）が既に持つ。
              「安心情報」を緑で出さない —— --text-muted の通常テキストで足りる */}
          <p className="delete-project-dialog__note">
            作業ツリーは残ります。消すにはカードの 🧹 を使ってください。
          </p>

          <label className="delete-project-dialog__confirm">
            <input
              type="checkbox"
              checked={confirmed}
              onChange={(e) => setConfirmed(e.target.checked)}
            />
            削除すると元に戻せないことを確認した
          </label>
        </div>

        <footer className="delete-project-dialog__footer">
          <button
            type="button"
            className="delete-project-dialog__button delete-project-dialog__button--ghost"
            onClick={onCancel}
          >
            キャンセル
          </button>
          <button
            type="button"
            className="delete-project-dialog__button delete-project-dialog__button--danger"
            disabled={!canDelete}
            onClick={onConfirm}
          >
            削除する
          </button>
        </footer>
      </div>
    </div>
  );
}
