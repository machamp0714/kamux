import { KANBAN_STATUSES, type KanbanStatus, type Session } from '../types/model';

/** 列ごとの表示順。契約 §10 の sessionOrder の型。 */
export type SessionOrder = Record<KanbanStatus, string[]>;

export const emptySessionOrder = (): Record<KanbanStatus, string[]> => ({
  backlog: [],
  in_progress: [],
  review: [],
  done: [],
});

/**
 * sessions から列の並びを導出する。sessionOrder は独立した可変状態ではなく、
 * sessions を変更したら必ずこの関数で作り直す。
 *
 * 並び規則: archived_at !== null を除外 → sort_order 昇順 → 同値なら id 辞書順。
 *
 * 前方参照（契約 §29.4）: M3-4 がここに `is_scratch` 除外を足す。
 * `is_scratch` は schema_version 3 で入るため M1-2 の時点では Session に
 * 存在せず、ここで先回りして書くとコンパイルが通らない。**M1-2 では書かないこと。**
 * id のタイブレークは、再採番の途中失敗などで sort_order 同値が到達しうるため必須
 * （無いと再レンダリングのたびに描画順が入れ替わりうる）。
 */
export const buildSessionOrder = (sessions: Record<string, Session>): SessionOrder => {
  const order = emptySessionOrder();
  for (const session of Object.values(sessions)) {
    if (session.archived_at !== null) continue;
    order[session.kanban_status].push(session.id);
  }
  for (const status of KANBAN_STATUSES) {
    order[status].sort((a, b) => {
      const diff = sessions[a].sort_order - sessions[b].sort_order;
      if (diff !== 0) return diff;
      return a < b ? -1 : a > b ? 1 : 0;
    });
  }
  return order;
};

/** セッション配列を「id 索引」と「列ごとの sort_order 昇順（同値は id 辞書順）」に畳む。 */
export const indexSessions = (
  list: Session[],
): { sessions: Record<string, Session>; sessionOrder: SessionOrder } => {
  const sessions = Object.fromEntries(list.map((s) => [s.id, s]));
  return { sessions, sessionOrder: buildSessionOrder(sessions) };
};
