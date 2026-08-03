import { create } from 'zustand';

import { ACTIVE_PROJECT_STORAGE_KEY, createProjectSlice, type ProjectSlice } from './projectSlice';
import { createSessionSlice, type SessionSlice } from './sessionSlice';
import { createTerminalSlice, type TerminalSlice } from './terminalSlice';
import { createUiSlice, type UiSlice } from './uiSlice';

export type AppStore = ProjectSlice & SessionSlice & UiSlice & TerminalSlice;

export const useAppStore = create<AppStore>()((...a) => ({
  ...createProjectSlice(...a),
  ...createSessionSlice(...a),
  ...createUiSlice(...a),
  ...createTerminalSlice(...a),
}));

/**
 * 起動時の復元シーケンス。App.tsx がマウント時に 1 度だけ呼ぶ。
 * localStorage の ID が消えたプロジェクトを指していても先頭に落として復帰する。
 */
export const bootstrap = async (): Promise<void> => {
  await useAppStore.getState().loadProjects();

  const { projects, setActiveProject } = useAppStore.getState();
  if (projects.length === 0) return;

  const saved = localStorage.getItem(ACTIVE_PROJECT_STORAGE_KEY);
  const target = projects.find((p) => p.id === saved) ?? projects[0];
  await setActiveProject(target.id);
};
