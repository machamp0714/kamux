import { useSortable } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { useAppStore } from '../../store';
import { toAppError } from '../../store/uiSlice';
import type { RuntimeState, Session } from '../../types/model';
import { KanbanCard } from './KanbanCard';

export interface SortableCardProps {
  session: Session;
  runtimeStates: Record<string, RuntimeState>;
}

export function SortableCard({ session, runtimeStates }: SortableCardProps) {
  const openModal = useAppStore((s) => s.openModal);
  const archiveSession = useAppStore((s) => s.archiveSession);
  const setError = useAppStore((s) => s.setError);
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: session.id,
  });

  return (
    <div
      ref={setNodeRef}
      className="kanban-sortable"
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
        opacity: isDragging ? 0.4 : 1,
      }}
      {...attributes}
      {...listeners}
    >
      <KanbanCard
        session={session}
        runtimeStates={runtimeStates}
        onEdit={(id) => openModal({ kind: 'edit_session', sessionId: id })}
        onArchive={(id) => {
          archiveSession(id).catch((e: unknown) => setError(toAppError(e)));
        }}
      />
    </div>
  );
}
