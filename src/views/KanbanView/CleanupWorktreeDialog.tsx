import { useEffect, useState } from 'react';

import type { RuntimeState, WorktreeStatus } from '../../types/model';
import './CleanupWorktreeDialog.css';

export interface CleanupWorktreeDialogProps {
  worktreePath: string;
  branch: string | null;
  /**
   * このセッションの現在の runtime_state。稼働中なら警告を出す（削除は止めない）。
   * `undefined` = 実行状態が未知（未起動 / 最初の `session://state` 到着前）。
   * **`session.last_runtime_state` で埋めてはならない**（契約 §38.3 論点 2 / §33.3 Q1）——
   * DB のスナップショットは現在の実行状態ではない。未知のときは何も主張せず警告を出さない
   */
  runtimeState: RuntimeState | undefined;
  /** null = 取得中 */
  status: WorktreeStatus | null;
  /** git / IPC の生メッセージ。加工しない */
  error: string | null;
  busy: boolean;
  onConfirm: (force: boolean) => void;
  onCancel: () => void;
  onOpenTerminal: () => void;
}

/**
 * worktree 削除の確認ダイアログ（純表示。ストアには触らない）。
 * ストアとの結線は `CleanupWorktreeDialogContainer` が持つ。
 *
 * **`aria-modal` は宣言しない。** `KanbanView/index.tsx` の `kanban-view__drawer` と同じ判断である ——
 * `aria-modal` は「外側は無効化されている」と宣言するが、本実装は外側を無効化していない
 * （focus trap も `inert` も無い）。加えて `cleanupDialog` は `uiSlice` の `modal` とは
 * 別フィールドなので `handleKeymapKeyDown`（`useKeymap.ts`）が組み立てる `modalOpen` 判定に
 * 乗らず、Escape で閉じる手段も無い。
 * 宣言と実態が食い違うと、支援技術の利用者は外側の内容を隠されたまま慣習的な閉じ方も持たない。
 * 閉じる手段はキャンセルボタンとスクリムの 2 つである。
 */
export function CleanupWorktreeDialog({
  worktreePath,
  branch,
  runtimeState,
  status,
  error,
  busy,
  onConfirm,
  onCancel,
  onOpenTerminal,
}: CleanupWorktreeDialogProps) {
  const [discardConfirmed, setDiscardConfirmed] = useState(false);
  const dirty = status?.dirty === true;
  // 稼働中の PTY の cwd を消すと、走っているプロセスの足元が抜ける。
  // ただし止めはしない — 判断はユーザーのもの（契約 §37.3「警告は出すが止めない」）。
  // undefined（実行状態が未知）は false 側に落とす。知らないときに「動いています」とは言わない（契約 §38.3）
  const live = runtimeState === 'running' || runtimeState === 'waiting_input';

  // 状態が clean → dirty に変わったときに、前のチェックを引き継がない
  useEffect(() => setDiscardConfirmed(false), [status]);

  const canDelete = status !== null && !busy && (!dirty || discardConfirmed);
  const label = dirty ? '強制削除する' : '削除する';
  const branchName = branch ?? '(なし)';

  return (
    <div className="cleanup-worktree-dialog__backdrop" onMouseDown={onCancel}>
      <div
        className="cleanup-worktree-dialog"
        role="dialog"
        aria-label="worktree を掃除"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="cleanup-worktree-dialog__header">
          {/* components.md「破壊的な確認ダイアログ」: 左肩に 32×32 のアイコン枠 */}
          <span className="cleanup-worktree-dialog__icon" aria-hidden="true">
            ⚠
          </span>
          <h2 className="cleanup-worktree-dialog__title">
            {dirty ? '未コミットの変更があります' : 'worktree を削除します'}
          </h2>
        </header>

        <div className="cleanup-worktree-dialog__body">
          <p className="cleanup-worktree-dialog__path">
            <code>{worktreePath}</code>
          </p>

          {live && (
            <p className="cleanup-worktree-dialog__warning" role="alert">
              このセッションはまだ動いています。削除すると実行中のプロセスの作業ディレクトリが消えます。
            </p>
          )}

          {status === null && error === null && (
            <p className="cleanup-worktree-dialog__loading">変更を確認しています…</p>
          )}

          {dirty && (
            <>
              {/* 危険の内訳は --bg-app + 1px solid --state-error のブロックへ入れる
                  （components.md「破壊的な確認ダイアログ」）。本文は赤くしない */}
              <ul className="cleanup-worktree-dialog__entries">
                {status?.entries.map((e) => (
                  <li key={e}>
                    <code>{e}</code>
                  </li>
                ))}
              </ul>
              <p className="cleanup-worktree-dialog__note">
                ブランチ <code>{branchName}</code> は残ります。あとで{' '}
                <code>git switch {branch ?? ''}</code> で作業を再開できます。
                ただし未コミットの変更はブランチには含まれていないため、削除すると復元できません。
              </p>
              <label className="cleanup-worktree-dialog__confirm">
                <input
                  type="checkbox"
                  checked={discardConfirmed}
                  onChange={(e) => setDiscardConfirmed(e.target.checked)}
                />
                変更を破棄して強制削除する
              </label>
              <button
                type="button"
                className="cleanup-worktree-dialog__button cleanup-worktree-dialog__button--ghost"
                onClick={onOpenTerminal}
              >
                ターミナルで確認する
              </button>
            </>
          )}

          {status !== null && !dirty && (
            <p className="cleanup-worktree-dialog__note">
              ブランチ <code>{branchName}</code> は残ります。あとで{' '}
              <code>git switch {branch ?? ''}</code> で作業を再開できます。
            </p>
          )}

          {/* 契約 §6: git / IPC の生メッセージを加工せず原文で出す */}
          {error !== null && <pre className="cleanup-worktree-dialog__error">{error}</pre>}
        </div>

        <footer className="cleanup-worktree-dialog__footer">
          <button
            type="button"
            className="cleanup-worktree-dialog__button cleanup-worktree-dialog__button--ghost"
            onClick={onCancel}
          >
            キャンセル
          </button>
          <button
            type="button"
            className="cleanup-worktree-dialog__button cleanup-worktree-dialog__button--danger"
            disabled={!canDelete}
            onClick={() => onConfirm(dirty)}
          >
            {label}
          </button>
        </footer>
      </div>
    </div>
  );
}
