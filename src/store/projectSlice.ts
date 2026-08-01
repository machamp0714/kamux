import type { StateCreator } from 'zustand';

import { createProject, listProjects } from '../ipc/commands';
import type { CliKind, Project } from '../types/model';
import type { AppStore } from './index';

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
    set({ activeProjectId: id });
    localStorage.setItem(ACTIVE_PROJECT_STORAGE_KEY, id);
    await get().loadSessions(id);
  },

  addProject: async (name, repoPath, defaultCli) => {
    const created = await createProject(name, repoPath, defaultCli);
    set({ projects: [...get().projects, created] });
    return created;
  },
});
