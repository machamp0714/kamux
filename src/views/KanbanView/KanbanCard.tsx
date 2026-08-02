import type { PointerEvent } from 'react';
import type { Session } from '../../types/model';
import { CLI_ICON } from './badge';

export interface KanbanCardProps {
  session: Session;
  onEdit?: (sessionId: string) => void;
  onArchive?: (sessionId: string) => void;
}

/** ボタン押下がドラッグ開始に化けないようにする（第1部 判断 7）。 */
function stopDrag(e: PointerEvent<HTMLButtonElement>) {
  e.stopPropagation();
}

/**
 * カードの見た目のみを持つ。dnd-kit に依存しないので DragOverlay でもそのまま使える。
 * M1-4 で onOpen（クリック → ターミナルへ）が追加される。
 */
export function KanbanCard({ session, onEdit, onArchive }: KanbanCardProps) {
  return (
    <article className="kanban-card" data-session-id={session.id}>
      <div className="kanban-card__head">
        {/* kanban-card__badge は M2-1 が <RuntimeBadge sessionId={session.id} /> で埋める */}
        <span className="kanban-card__cli" title={session.cli_kind} aria-label={session.cli_kind}>
          {CLI_ICON[session.cli_kind]}
        </span>
        <h3 className="kanban-card__title">{session.title}</h3>
      </div>

      {session.description !== '' ? (
        <p className="kanban-card__description">{session.description}</p>
      ) : null}

      {session.mode === 'worktree' && session.branch !== null ? (
        <p className="kanban-card__branch" title={session.branch}>
          {session.branch}
        </p>
      ) : null}

      <div className="kanban-card__actions">
        <button type="button" onPointerDown={stopDrag} onClick={() => onEdit?.(session.id)}>
          編集
        </button>
        <button type="button" onPointerDown={stopDrag} onClick={() => onArchive?.(session.id)}>
          アーカイブ
        </button>
      </div>
    </article>
  );
}
