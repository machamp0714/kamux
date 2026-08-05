import { useSortable } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { useAppStore } from '../../store';
import { toAppError } from '../../store/uiSlice';
import type { Session } from '../../types/model';
import { KanbanCard } from './KanbanCard';

export interface SortableCardProps {
  session: Session;
}

export function SortableCard({ session }: SortableCardProps) {
  const openModal = useAppStore((s) => s.openModal);
  const archiveSession = useAppStore((s) => s.archiveSession);
  const setError = useAppStore((s) => s.setError);
  const focusSession = useAppStore((s) => s.focusSession);
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({
    id: session.id,
    // dnd-kit の既定 role='button' は使わない。カードは「編集」「アーカイブ」の
    // <button> を内包する（role=button は children-presentational）うえ、Space は
    // sensors.ts でドラッグ開始に割り当てられていて button のキーボード契約とも
    // 食い違う。JSX 側で role を消すのではなくここで指定するのは、dnd-kit が
    // role === 'button' のときだけ aria-pressed を立てるためである
    // （@dnd-kit/core の useDraggable: `isDragging && role === defaultRole`）。
    attributes: { role: 'article' },
  });

  // 活性化属性・リスナはラッパではなくカード本体（.kanban-card）に載せる。
  // ラッパに載せるとタブストップが 2 つになり、先頭のストップでは Enter も
  // :focus-within も効かない。ラッパは変形（transform）専用にする。
  return (
    <div
      ref={setNodeRef}
      className="kanban-sortable"
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
        opacity: isDragging ? 0.4 : 1,
      }}
    >
      <KanbanCard
        session={session}
        onOpen={(id) => focusSession(id, 'terminal')}
        dragActivator={{ ref: setActivatorNodeRef, attributes, listeners }}
        onEdit={(id) => openModal({ kind: 'edit_session', sessionId: id })}
        onArchive={(id) => {
          archiveSession(id).catch((e: unknown) => setError(toAppError(e)));
        }}
      />
    </div>
  );
}
