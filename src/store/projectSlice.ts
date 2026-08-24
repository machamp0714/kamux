import type { StateCreator } from 'zustand';

import { createProject, deleteProject, listProjects, stopSession } from '../ipc/commands';
import type { CliKind, Layout, Project } from '../types/model';
import type { AppStore } from './index';
import { emptySessionOrder } from './kanbanOrder';
import { withFocus } from './terminalSlice';
import { isLayout } from './paneLogic';

/**
 * 最後に選択したプロジェクト。UI の状態であってドメインデータではないため
 * DB ではなく localStorage に置く。値が古い/消えた場合は先頭プロジェクトに落とす
 * （その復元シーケンスは Task 18 の `bootstrap()` の担当。ここでは持たない）。
 */
export const ACTIVE_PROJECT_STORAGE_KEY = 'kamux.activeProjectId';

export interface ProjectSlice {
  projects: Project[];
  activeProjectId: string | null;
  loadProjects: () => Promise<void>;
  setActiveProject: (id: string) => Promise<void>;
  addProject: (name: string, repoPath: string, defaultCli: CliKind) => Promise<Project>;
  /**
   * プロジェクトを削除する（契約 §130.4 / §130.5）。確認ダイアログを通した後に呼ぶこと
   * （契約 §7.1。押した瞬間には呼ばない）。
   *
   * `sessionIds` は止める対象で、呼び出し側（確認ダイアログ）が `list_sessions` で
   * 取ったものを渡す。🔴 ここで `get().sessions` から引き直さないこと —— `loadSessions`
   * は「置換ではなくマージ」なので、一度も開いていないプロジェクトのセッションは
   * ストアに 1 件も載っておらず、無差別に回すはずの `stop_session` が 0 件になる。
   */
  removeProject: (id: string, sessionIds: string[]) => Promise<void>;
}

export const createProjectSlice: StateCreator<AppStore, [], [], ProjectSlice> = (set, get) => ({
  projects: [],
  activeProjectId: null,

  loadProjects: async () => {
    const projects = await listProjects();
    set({ projects });
  },

  setActiveProject: async (id) => {
    const prev = get();

    // 1. 現在のプロジェクトのターミナルワークスペースを退避する
    const workspaceByProject = { ...prev.workspaceByProject };
    if (prev.activeProjectId !== null) {
      workspaceByProject[prev.activeProjectId] = {
        layout: prev.layout,
        paneAssignment: prev.paneAssignment,
        activePane: prev.activePane,
      };
    }
    set({ workspaceByProject, activeProjectId: id, focusedSessionId: null });
    localStorage.setItem(ACTIVE_PROJECT_STORAGE_KEY, id);

    // 2. ボードを差し替える。PTY には一切触らない（stop_session を呼ばない）
    await get().loadSessions(id);

    // 3. 切替先のワークスペースを復元する。無ければ先頭セッションを 1 面表示
    //    §85.1 の不変条件（focusedSessionId === paneAssignment[activePane]）を
    //    維持するため、withFocus() でラップしてから set() に渡す。
    set((st) => {
      const saved = st.workspaceByProject[id];
      if (saved) {
        // 契約 §28.6: 永続化された layout は検証してから流す。旧バージョンが
        // 書いた JSON や未知の値をそのまま set() すると、型に無い値がストアに入る
        const layout: Layout = isLayout(saved.layout) ? saved.layout : 'single';
        return withFocus({
          layout,
          paneAssignment: saved.paneAssignment,
          activePane: saved.activePane,
        });
      }
      const first =
        Object.values(st.sessions).find((x) => x.project_id === id && x.archived_at === null)?.id ??
        null;
      return withFocus({
        layout: 'single',
        paneAssignment: [first, null],
        activePane: 0,
      });
    });
  },

  addProject: async (name, repoPath, defaultCli) => {
    const created = await createProject(name, repoPath, defaultCli);
    set({ projects: [...get().projects, created] });
    return created;
  },

  removeProject: async (id, sessionIds) => {
    // 1. 契約 §130.4: sessions は §3 の ON DELETE CASCADE で消える。行だけが消えて
    //    PTY が生き残ると、どのカードからも辿れない孤児になる。
    //    🔴 稼働中かどうかで分岐しない。stop_session は冪等（契約 §15 / session/mod.rs の
    //    stop_agent_surface）なので、対象プロジェクトの全セッションへ無差別に回す。
    //    契約 §147.2: runtimeStates の購読を停止処理の分岐に使わないこと ——
    //    使うと「稼働中でないから止めない」という分岐が生まれ、冪等性が担保している
    //    安全性を捨てることになる。
    //    対象は呼び出し側が list_sessions で取った sessionIds である（doc 参照）。
    await Promise.all(sessionIds.map((sid) => stopSession(sid)));

    // 2. worktree は消さない（契約 §130.4）。§13 が「git branch -D は決して実行しない」と
    //    定めているのと同じ性格の判断で、消す導線は 🧹 = plan_cleanup が既に持つ。
    await deleteProject(id);

    const remaining = get().projects.filter((p) => p.id !== id);
    set({ projects: remaining });

    // 3. 契約 §130.5 の 3 ケース。非アクティブなら activeProjectId は動かさない。
    if (get().activeProjectId !== id) return;

    if (remaining.length === 0) {
      // 落とし先が無いので setActiveProject を通せない唯一の経路。盤面を空にする
      // （§85.1 の focusedSessionId === paneAssignment[activePane] は withFocus が守る）。
      set({
        ...withFocus({ layout: 'single', paneAssignment: [null, null], activePane: 0 }),
        activeProjectId: null,
        sessionOrder: emptySessionOrder(),
      });
      return;
    }

    // 残りの先頭へ。set({ activeProjectId }) だけで済ませないのは、ワークスペースの退避・
    // loadSessions・ペイン復元・§85.1 の維持を setActiveProject が持っているためである。
    await get().setActiveProject(remaining[0].id);
  },
});
