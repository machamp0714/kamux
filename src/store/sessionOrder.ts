import type { KanbanStatus, Session } from '../types/model';

export { KANBAN_STATUSES } from '../types/model';

/**
 * ボード 1 枚分の表示順を作る。
 * `sessions` はプロジェクト横断のグローバルキャッシュを渡してよい。
 * アクティブプロジェクト以外と、アーカイブ済み（archived_at !== null）は除外する。
 */
export function buildSessionOrder(
  sessions: Session[],
  projectId: string,
): Record<KanbanStatus, string[]> {
  const order: Record<KanbanStatus, string[]> = {
    backlog: [],
    in_progress: [],
    review: [],
    done: [],
  };
  sessions
    .filter((x) => x.project_id === projectId && x.archived_at === null)
    .slice()
    .sort((a, b) => a.sort_order - b.sort_order)
    .forEach((x) => order[x.kanban_status].push(x.id));
  return order;
}
