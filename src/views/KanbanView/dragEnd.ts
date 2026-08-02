import type { SessionOrder } from '../../store/kanbanOrder';
import { KANBAN_STATUSES, type KanbanStatus } from '../../types/model';
import { isKanbanStatus } from './columns';

/** 列そのものを droppable にするための id 接頭辞。カード id（UUID）と衝突しない。 */
export const COLUMN_DROPPABLE_PREFIX = 'column:';

export function columnDroppableId(status: KanbanStatus): string {
  return `${COLUMN_DROPPABLE_PREFIX}${status}`;
}

export function parseColumnDroppableId(id: string): KanbanStatus | null {
  if (!id.startsWith(COLUMN_DROPPABLE_PREFIX)) return null;
  const rest = id.slice(COLUMN_DROPPABLE_PREFIX.length);
  return isKanbanStatus(rest) ? rest : null;
}

export interface DragEndResult {
  to: KanbanStatus;
  /** 移動前の配列における挿入位置。moveCardInOrder の index 規約と同じ */
  index: number;
}

/**
 * dnd-kit の DragEndEvent を moveCard の引数へ落とす純関数。
 * 返す index は「移動前の order における over カードの位置」で、
 * moveCardInOrder（remove → insert）と組み合わせて arrayMove と同じ結果になる。
 */
export function resolveDragEnd(
  activeId: string,
  overId: string | null,
  order: SessionOrder,
): DragEndResult | null {
  if (overId === null) return null;
  if (overId === activeId) return null;

  const column = parseColumnDroppableId(overId);
  if (column !== null) {
    return { to: column, index: order[column].length };
  }

  for (const status of KANBAN_STATUSES) {
    const index = order[status].indexOf(overId);
    if (index !== -1) return { to: status, index };
  }
  return null;
}
