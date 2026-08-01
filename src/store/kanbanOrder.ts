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

/**
 * 列間・列内の移動後の SessionOrder を返す。入力は破壊しない。
 *
 * index の意味は「移動前の配列における over カードの位置」。remove → insert の順で
 * 処理するため、この規約のもとで dnd-kit の arrayMove(items, from, to) と一致する。
 *
 * 除去は移動元の列だけでなく **全列** を走査する。M1-1 の moveCard が防御的に
 * そうしていたので挙動を落とさない（何らかの理由で 2 列に同じ id が入っていても
 * 移動後に重複が残らない）。移動元の列は id から一意に決まるので引数に取らない。
 */
export function moveCardInOrder(
  order: SessionOrder,
  sessionId: string,
  to: KanbanStatus,
  index: number,
): SessionOrder {
  const next: SessionOrder = {
    backlog: [...order.backlog],
    in_progress: [...order.in_progress],
    review: [...order.review],
    done: [...order.done],
  };
  for (const status of KANBAN_STATUSES) {
    const at = next[status].indexOf(sessionId);
    if (at !== -1) next[status].splice(at, 1);
  }
  // 契約 §49.3.2: index が L.len() を超えたら末尾へクランプする。
  // サーバ側（契約 §7.4 の `to_index >= L.len()` の枝）も独立に同じクランプを行う。
  const insertAt = Math.max(0, Math.min(index, next[to].length));
  next[to].splice(insertAt, 0, sessionId);
  return next;
}
