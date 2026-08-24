import type { StateCreator } from 'zustand';

import { createProject, listProjects } from '../ipc/commands';
import type { CliKind, Layout, Project } from '../types/model';
import type { AppStore } from './index';
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
});
