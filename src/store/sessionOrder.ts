import type { KanbanStatus, Session } from '../types/model';

export { KANBAN_STATUSES } from '../types/model';

/**
 * 契約 §50.3.2 の並び順の全順序: sort_order 昇順、同値なら id 昇順でタイブレークする。
 * 本番の全列構築（buildSessionOrder）と restoreSession の挿入位置探索（sessionSlice.ts）
 * の両方がこの 1 本を共有する（契約 §144.7 / 裁定 72）。2 箇所へ別々に書くと、
 * 挿入位置が土台の作られ方に依存してしまう。
 */
export function compareSessionOrder(a: Session, b: Session): number {
  const diff = a.sort_order - b.sort_order;
  if (diff !== 0) return diff;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

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
    .sort(compareSessionOrder)
    .forEach((x) => order[x.kanban_status].push(x.id));
  return order;
}
