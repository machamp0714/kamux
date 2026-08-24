import type { Session } from '../../types/model';
import { isCleanupSuggested } from '../../store/cleanup';
import './ArchivedDrawer.css';

export interface ArchivedDrawerProps {
  open: boolean;
  /** アクティブプロジェクトの全セッション（アーカイブ済みを含む） */
  sessions: Session[];
  onRestore: (id: string) => void;
  onCleanup: (id: string) => void;
  onClose: () => void;
}

export function ArchivedDrawer({
  open,
  sessions,
  onRestore,
  onCleanup,
  onClose,
}: ArchivedDrawerProps): JSX.Element | null {
  if (!open) return null;

  // filter は新しい配列を返すので、sort の破壊性に対する防御として .slice() を
  // 挟む必要は無い（裁定 67）。新しい順（archived_at 降順）に並べる。
  const archived = sessions
    .filter((s) => s.archived_at !== null)
    .sort((a, b) => (b.archived_at ?? 0) - (a.archived_at ?? 0));

  return (
    <div className="archived-drawer-scrim" onMouseDown={onClose}>
      <aside
        className="archived-drawer"
        aria-label="アーカイブ済み"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="archived-drawer__header">
          <h2 className="archived-drawer__title">アーカイブ済み</h2>
          <button type="button" className="archived-drawer__close" onClick={onClose}>
            閉じる
          </button>
        </header>
        {archived.length === 0 ? (
          <p className="archived-drawer__empty">アーカイブ済みのセッションはありません</p>
        ) : (
          <ul className="archived-drawer__list">
            {archived.map((s) => (
              <li key={s.id} className="archived-drawer__item">
                <span className="archived-drawer__title-text">{s.title}</span>
                <div className="archived-drawer__actions">
                  <button
                    type="button"
                    className="archived-drawer__restore"
                    onClick={() => onRestore(s.id)}
                  >
                    復元
                  </button>
                  {isCleanupSuggested(s) && (
                    // 裁定 62: KanbanCardCleanup.tsx（カード側の同機能ボタン）と
                    // 同じ aria-label を持たせる。可視ラベルは同じ 🧹 worktree のまま。
                    <button
                      type="button"
                      className="archived-drawer__cleanup"
                      title="worktree を掃除"
                      aria-label="worktree を掃除"
                      onClick={() => onCleanup(s.id)}
                    >
                      🧹 worktree
                    </button>
                  )}
                </div>
              </li>
            ))}
          </ul>
        )}
      </aside>
    </div>
  );
}
