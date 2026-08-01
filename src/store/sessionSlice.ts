import type { StateCreator } from 'zustand';

import {
  createSession,
  listSessions,
  moveSession,
  updateSession as updateSessionCmd,
  type CreateSessionArgs,
} from '../ipc/commands';
import type { KanbanStatus, RuntimeState, Session, SessionPatch } from '../types/model';
import { emptySessionOrder, indexSessions, moveCardInOrder } from './kanbanOrder';
import type { AppStore } from './index';

export { emptySessionOrder, indexSessions } from './kanbanOrder';

export interface SessionSlice {
  sessions: Record<string, Session>;
  sessionOrder: Record<KanbanStatus, string[]>;

  /**
   * runtime バッジの表示枠。M1-2 は型と初期値だけを置き、書き換えない。
   * 値の導出（applyStateEvent）は M2-1 の担当（契約 §2 / §10）。
   */
  runtimeStates: Record<string, RuntimeState>;

  loadSessions: (projectId: string) => Promise<void>;
  addSession: (args: CreateSessionArgs) => Promise<Session>;
  moveCard: (sessionId: string, to: KanbanStatus, index: number) => Promise<void>;
  editSession: (id: string, patch: SessionPatch) => Promise<Session>;
  archiveSession: (id: string) => Promise<void>;
}

export const createSessionSlice: StateCreator<AppStore, [], [], SessionSlice> = (set, get) => ({
  sessions: {},
  sessionOrder: emptySessionOrder(),
  runtimeStates: {},

  loadSessions: async (projectId) => {
    // アーカイブ済みは表示しない（復活 UX は M3-4）
    const list = await listSessions(projectId, false);
    // 応答が返るまでにプロジェクトが切り替わっていたら捨てる（M1-1 からの申し送り）。
    // sessions / sessionOrder を所有する sessionSlice が「これらは常に activeProjectId の
    // ものである」という不変条件も持つ。projectSlice.setActiveProject は
    // activeProjectId を set してから loadSessions を await するので、初回ロードは弾かれない。
    if (get().activeProjectId !== projectId) return;
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

  editSession: async (id, patch) => {
    // title / description しか変えない想定（判断 10）。並びに影響しないので
    // sessionOrder は触らず、sessions の当該エントリだけを差し替える。
    const saved = await updateSessionCmd(id, patch);
    set({ sessions: { ...get().sessions, [saved.id]: saved } });
    return saved;
  },

  archiveSession: async (id) => {
    const snapshot = get().sessions;
    const prevOrder = get().sessionOrder;
    const target = snapshot[id];
    if (target === undefined) return;

    // 楽観更新: 盤面からは当該列だけ除去する（buildSessionOrder による全列再構築は
    // moveCard の in-flight 中と重なると、移動中のカードが古い sort_order の位置へ
    // 吸着して見えるおそれがあるため避ける）。
    const archivedAt = Date.now();
    const optimisticSessions = { ...snapshot, [id]: { ...target, archived_at: archivedAt } };
    const optimisticOrder = {
      ...prevOrder,
      [target.kanban_status]: prevOrder[target.kanban_status].filter((sid) => sid !== id),
    };
    set({ sessions: optimisticSessions, sessionOrder: optimisticOrder });

    try {
      const saved = await updateSessionCmd(id, { archived_at: archivedAt });
      set({ sessions: { ...get().sessions, [saved.id]: saved } });
    } catch (e) {
      set({ sessions: snapshot, sessionOrder: prevOrder });
      throw e;
    }
  },
});
