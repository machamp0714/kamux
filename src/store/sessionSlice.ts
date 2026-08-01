import type { StateCreator } from 'zustand';

import { createSession, listSessions, type CreateSessionArgs } from '../ipc/commands';
import type { KanbanStatus, Session } from '../types/model';
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

export interface SessionSlice {
  sessions: Record<string, Session>;
  sessionOrder: Record<KanbanStatus, string[]>;
  loadSessions: (projectId: string) => Promise<void>;
  addSession: (args: CreateSessionArgs) => Promise<Session>;
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
});
