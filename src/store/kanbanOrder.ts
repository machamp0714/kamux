import type { KanbanStatus, Session } from '../types/model';

/** 列ごとの表示順。契約 §10 の sessionOrder の型。 */
export type SessionOrder = Record<KanbanStatus, string[]>;

export const emptySessionOrder = (): Record<KanbanStatus, string[]> => ({
  backlog: [],
  in_progress: [],
  review: [],
  done: [],
});

/** セッション配列を「id 索引」と「列ごとの sort_order 昇順」に畳む。
 * sort_order が同値の場合は Array.prototype.sort の安定性（ES2019+）に頼って入力順を保つ。
 * 入力は Rust 側の list_sessions（ORDER BY kanban_status, sort_order, id）で
 * すでに id 昇順に決着しているため、フロント側で id による再ソートはしない。
 */
export const indexSessions = (
  list: Session[],
): { sessions: Record<string, Session>; sessionOrder: Record<KanbanStatus, string[]> } => {
  const sessions: Record<string, Session> = {};
  const sessionOrder = emptySessionOrder();

  for (const s of [...list].sort((a, b) => a.sort_order - b.sort_order)) {
    sessions[s.id] = s;
    sessionOrder[s.kanban_status].push(s.id);
  }

  return { sessions, sessionOrder };
};
