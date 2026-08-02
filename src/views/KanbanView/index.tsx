import { useState } from 'react';
import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  closestCorners,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from '@dnd-kit/core';
import { sortableKeyboardCoordinates } from '@dnd-kit/sortable';
import { useAppStore } from '../../store';
import { toAppError } from '../../store/uiSlice';
import { KANBAN_STATUSES } from '../../types/model';
import { KanbanCard } from './KanbanCard';
import { KanbanColumn } from './KanbanColumn';
import { resolveDragEnd } from './dragEnd';
import { KANBAN_KEYBOARD_CODES, KANBAN_POINTER_ACTIVATION_DISTANCE } from './sensors';
import './kanban.css';

export function KanbanView() {
  const sessions = useAppStore((s) => s.sessions);
  const sessionOrder = useAppStore((s) => s.sessionOrder);
  const runtimeStates = useAppStore((s) => s.runtimeStates);
  const moveCard = useAppStore((s) => s.moveCard);
  const openModal = useAppStore((s) => s.openModal);
  const setError = useAppStore((s) => s.setError);
  const [draggingId, setDraggingId] = useState<string | null>(null);

  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: { distance: KANBAN_POINTER_ACTIVATION_DISTANCE },
    }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
      keyboardCodes: KANBAN_KEYBOARD_CODES,
    }),
  );

  const onDragStart = (event: DragStartEvent) => setDraggingId(String(event.active.id));

  const onDragEnd = (event: DragEndEvent) => {
    setDraggingId(null);
    const activeId = String(event.active.id);
    const overId = event.over === null ? null : String(event.over.id);
    const result = resolveDragEnd(activeId, overId, sessionOrder);
    if (result === null) return;
    // moveCard は失敗時に巻き戻して rethrow する（M1-1 の契約）。
    // エラーの提示は呼び出し側の責務（第1部 判断 2）
    moveCard(activeId, result.to, result.index).catch((e: unknown) => setError(toAppError(e)));
  };

  const dragging = draggingId === null ? undefined : sessions[draggingId];

  return (
    <div className="kanban-view">
      <header className="kanban-view__header">
        <h1 className="kanban-view__heading">カンバン</h1>
        <button
          type="button"
          className="kanban-view__new"
          onClick={() => openModal({ kind: 'create_session' })}
        >
          新規セッション <kbd>⌘N</kbd>
        </button>
      </header>

      <DndContext
        sensors={sensors}
        collisionDetection={closestCorners}
        onDragStart={onDragStart}
        onDragEnd={onDragEnd}
        onDragCancel={() => setDraggingId(null)}
      >
        <div className="kanban-view__board">
          {KANBAN_STATUSES.map((status) => (
            <KanbanColumn
              key={status}
              status={status}
              sessionIds={sessionOrder[status]}
              sessions={sessions}
              runtimeStates={runtimeStates}
            />
          ))}
        </div>
        <DragOverlay>
          {dragging === undefined ? null : (
            <KanbanCard session={dragging} runtimeStates={runtimeStates} />
          )}
        </DragOverlay>
      </DndContext>
    </div>
  );
}
