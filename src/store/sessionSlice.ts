import type { StateCreator } from 'zustand';

import {
  createSession,
  listSessions,
  updateSession,
  type CreateSessionArgs,
} from '../ipc/commands';
import { KANBAN_STATUSES, type KanbanStatus, type Session } from '../types/model';
import type { AppStore } from './index';

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

// 契約 §7.4 / §44.5: sort_order の採番は M1-2 で move_session（サーバ側・原子的）に移る。
// M1-1 が中点計算をフロントに置くのは、再起動復元の検証に並びの永続化が要るため。

/**
 * 契約 §3: sort_order は REAL。列内 DnD では両隣の中点を書くだけで済ませ、
 * 他行の再採番 UPDATE を発生させない。
 * neighbors は「移動するカード自身を除いた」移動先の列の sort_order 昇順配列。
 */
export const computeSortOrder = (neighbors: number[], index: number): number => {
  const before = index > 0 ? neighbors[index - 1] : undefined;
  const after = index < neighbors.length ? neighbors[index] : undefined;

  if (before === undefined && after === undefined) return 1;
  if (before === undefined) return (after as number) - 1;
  if (after === undefined) return before + 1;
  return (before + after) / 2;
};

export interface SessionSlice {
  sessions: Record<string, Session>;
  sessionOrder: Record<KanbanStatus, string[]>;
  loadSessions: (projectId: string) => Promise<void>;
  addSession: (args: CreateSessionArgs) => Promise<Session>;
  moveCard: (sessionId: string, to: KanbanStatus, index: number) => Promise<void>;
}

export const createSessionSlice: StateCreator<AppStore, [], [], SessionSlice> = (set, get) => ({
  sessions: {},
  sessionOrder: emptySessionOrder(),

  loadSessions: async (projectId) => {
    // アーカイブ済みは表示しない（復活 UX は M3-4）
    const list = await listSessions(projectId, false);
    set(indexSessions(list));
  },

  addSession: async (args) => {
    const created = await createSession(args);
    const { sessions, sessionOrder } = get();
    const column = sessionOrder[created.kanban_status];
    set({
      sessions: { ...sessions, [created.id]: created },
      sessionOrder: { ...sessionOrder, [created.kanban_status]: [...column, created.id] },
    });
    return created;
  },

  moveCard: async (sessionId, to, index) => {
    const { sessions, sessionOrder } = get();
    const target = sessions[sessionId];
    if (!target) return;

    const remaining = sessionOrder[to].filter((id) => id !== sessionId);
    const neighbors = remaining.map((id) => sessions[id].sort_order);
    const sortOrder = computeSortOrder(neighbors, index);

    // 楽観更新: DnD の手応えを IPC の往復で待たせない
    const nextOrder = emptySessionOrder();
    for (const status of KANBAN_STATUSES) {
      nextOrder[status] = sessionOrder[status].filter((id) => id !== sessionId);
    }
    nextOrder[to] = [...nextOrder[to].slice(0, index), sessionId, ...nextOrder[to].slice(index)];

    set({
      sessions: {
        ...sessions,
        [sessionId]: { ...target, kanban_status: to, sort_order: sortOrder },
      },
      sessionOrder: nextOrder,
    });

    // 確定: DB が返した行で上書きする。
    // 失敗したら楽観更新を巻き戻す（DB が受け付けなかった位置にカードを残さない）。
    try {
      const saved = await updateSession(sessionId, { kanban_status: to, sort_order: sortOrder });
      set({ sessions: { ...get().sessions, [saved.id]: saved } });
    } catch (e) {
      set({ sessions, sessionOrder });
      throw e;
    }
  },
});
