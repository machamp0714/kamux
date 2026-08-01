import type { StateCreator } from 'zustand';

import { createSession, listSessions, moveSession, type CreateSessionArgs } from '../ipc/commands';
import type { KanbanStatus, Session } from '../types/model';
import { emptySessionOrder, indexSessions, moveCardInOrder } from './kanbanOrder';
import type { AppStore } from './index';

export { emptySessionOrder, indexSessions } from './kanbanOrder';

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

    // 楽観更新: 配列の並べ替えだけを行う。sort_order の実値は算出しない（契約 §7.4）。
    // DnD の手応えを IPC の往復で待たせないため（判断 3）。
    const nextOrder = moveCardInOrder(sessionOrder, sessionId, to, index);
    set({
      sessions: { ...sessions, [sessionId]: { ...target, kanban_status: to } },
      sessionOrder: nextOrder,
    });

    try {
      // 戻り値は「移動先の列」の全 Session（sort_order 昇順・同値は id タイブレーク。契約 §49.4）。
      // 移動元の列は 1 行も変化しないので返らない。楽観更新で除去済みの状態が正しい。
      const column = await moveSession(sessionId, to, index);
      const merged = { ...get().sessions };
      for (const s of column) merged[s.id] = s;
      set({
        sessions: merged,
        sessionOrder: { ...get().sessionOrder, [to]: column.map((s) => s.id) },
      });
    } catch (e) {
      // DB が受け付けなかった位置にカードを残さない
      set({ sessions, sessionOrder });
      throw e;
    }
  },
});
