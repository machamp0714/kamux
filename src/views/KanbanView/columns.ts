import { KANBAN_STATUSES, type KanbanStatus } from '../../types/model';

/** 設計書 §6.2 の 4 列の表示ラベル。表示順は KANBAN_STATUSES（M1-1）が正典。 */
export const COLUMN_LABELS: Record<KanbanStatus, string> = {
  backlog: 'Backlog',
  in_progress: 'In Progress',
  review: 'Review',
  done: 'Done',
};

export function isKanbanStatus(value: string): value is KanbanStatus {
  return (KANBAN_STATUSES as string[]).includes(value);
}
