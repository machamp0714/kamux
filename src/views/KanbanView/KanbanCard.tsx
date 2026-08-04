import type { KeyboardEvent, MouseEvent, PointerEvent } from 'react';
import { useAppStore } from '../../store';
import type { RuntimeState, Session } from '../../types/model';
import { CLI_ICON, runtimeBadge } from './badge';

export interface KanbanCardProps {
  session: Session;
  runtimeStates: Record<string, RuntimeState>;
  onEdit?: (sessionId: string) => void;
  onArchive?: (sessionId: string) => void;
}

/** ボタン押下がドラッグ開始に化けないようにする（第1部 判断 7）。 */
function stopDrag(e: PointerEvent<HTMLButtonElement>) {
  e.stopPropagation();
}

/**
 * カード内のボタン（編集 / アーカイブ）のクリックがカード自体の
 * onClick（focusSession）へバブリングしないようにする（要件5）。
 */
function stopOpen(e: MouseEvent<HTMLButtonElement>) {
  e.stopPropagation();
}

/**
 * カードの見た目のみを持つ。dnd-kit に依存しないので DragOverlay でもそのまま使える。
 * クリック / Enter でターミナル画面の該当ペインへフォーカスする（要件5・契約 §11）。
 */
export function KanbanCard({ session, runtimeStates, onEdit, onArchive }: KanbanCardProps) {
  const badge = runtimeBadge(runtimeStates, session.id);
  const focusSession = useAppStore((s) => s.focusSession);

  const open = () => focusSession(session.id, 'terminal');

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key !== 'Enter') return;
    e.preventDefault();
    open();
  };

  return (
    <article
      className="kanban-card"
      data-session-id={session.id}
      role="button"
      tabIndex={0}
      onClick={open}
      onKeyDown={onKeyDown}
    >
      <div className="kanban-card__head">
        {badge !== null && (
          <span className="kanban-card__badge" title={badge.label} aria-label={badge.label}>
            {badge.glyph}
          </span>
        )}
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
        <button
          type="button"
          onPointerDown={stopDrag}
          onClick={(e) => {
            stopOpen(e);
            onEdit?.(session.id);
          }}
        >
          編集
        </button>
        <button
          type="button"
          onPointerDown={stopDrag}
          onClick={(e) => {
            stopOpen(e);
            onArchive?.(session.id);
          }}
        >
          アーカイブ
        </button>
      </div>
    </article>
  );
}
