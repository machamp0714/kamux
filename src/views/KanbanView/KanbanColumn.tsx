import { useDroppable } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy } from '@dnd-kit/sortable';
import type { KanbanStatus, Session } from '../../types/model';
import { COLUMN_LABELS } from './columns';
import { columnDroppableId } from './dragEnd';
import { SortableCard } from './SortableCard';

export interface KanbanColumnProps {
  status: KanbanStatus;
  sessionIds: string[];
  sessions: Record<string, Session>;
}

export function KanbanColumn({ status, sessionIds, sessions }: KanbanColumnProps) {
  // 空の列にもドロップできるよう、列そのものを droppable にする
  const { setNodeRef, isOver } = useDroppable({ id: columnDroppableId(status) });

  return (
    <section className="kanban-column" aria-label={COLUMN_LABELS[status]}>
      <h2 className="kanban-column__title">
        {COLUMN_LABELS[status]}
        <span className="kanban-column__count">{sessionIds.length}</span>
      </h2>
      <SortableContext items={sessionIds} strategy={verticalListSortingStrategy}>
        <div
          ref={setNodeRef}
          className={`kanban-column__list${isOver ? ' kanban-column__list--over' : ''}`}
          data-column={status}
        >
          {sessionIds.map((id) => (
            <SortableCard key={id} session={sessions[id]} />
          ))}
        </div>
      </SortableContext>
    </section>
  );
}
