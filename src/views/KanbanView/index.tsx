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
import { HooksStatusPanel } from '../../components/HooksStatusPanel';
import { KANBAN_STATUSES, type Session } from '../../types/model';
import { KanbanCard } from './KanbanCard';
import { KanbanColumn } from './KanbanColumn';
import { resolveDragEnd } from './dragEnd';
import { KANBAN_KEYBOARD_CODES, KANBAN_POINTER_ACTIVATION_DISTANCE } from './sensors';
import './kanban.css';

/**
 * HooksStatusPanel は全セッション横断のパネルなので、activeProjectId で絞らず
 * ストアの sessions を全件そのまま渡す（lane-controller の統合裁定）。
 */
function toSessionTitles(sessions: Record<string, Session>): Record<string, string> {
  const titles: Record<string, string> = {};
  for (const s of Object.values(sessions)) titles[s.id] = s.title;
  return titles;
}

export function KanbanView() {
  const sessions = useAppStore((s) => s.sessions);
  const sessionOrder = useAppStore((s) => s.sessionOrder);
  const moveCard = useAppStore((s) => s.moveCard);
  const openModal = useAppStore((s) => s.openModal);
  const setError = useAppStore((s) => s.setError);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  // ドロワーの開閉はこのビューのローカル state に置く（uiSlice には足さない。
  // M3-4 の ArchivedDrawer が同じファイルへ showArchived を足す予定のため）。
  const [hooksOpen, setHooksOpen] = useState(false);

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
        <div className="kanban-view__actions">
          <button type="button" className="kanban-view__hooks" onClick={() => setHooksOpen(true)}>
            hooks 疎通ステータス
          </button>
          <button
            type="button"
            className="kanban-view__new"
            onClick={() => openModal({ kind: 'create_session' })}
          >
            新規セッション <kbd>⌘N</kbd>
          </button>
        </div>
      </header>

      {/* パネルは開いている間だけマウントする。マウント時に 1 回だけ取得する設計
          （HooksStatusPanel 参照）なので、開くたびに最新の診断が読まれる。 */}
      {hooksOpen ? (
        <div className="kanban-view__drawer-scrim" onMouseDown={() => setHooksOpen(false)}>
          <aside
            className="kanban-view__drawer"
            role="dialog"
            aria-modal="true"
            aria-label="hooks 疎通ステータス"
            onMouseDown={(e) => e.stopPropagation()}
          >
            <div className="kanban-view__drawer-header">
              <button
                type="button"
                className="kanban-view__drawer-close"
                onClick={() => setHooksOpen(false)}
              >
                閉じる
              </button>
            </div>
            <HooksStatusPanel sessionTitles={toSessionTitles(sessions)} />
          </aside>
        </div>
      ) : null}

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
            />
          ))}
        </div>
        <DragOverlay>
          {dragging === undefined ? null : <KanbanCard session={dragging} />}
        </DragOverlay>
      </DndContext>
    </div>
  );
}
