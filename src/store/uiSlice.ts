import type { StateCreator } from 'zustand';

import type { AppStore } from './index';

export interface UiSlice {
  view: 'kanban' | 'terminal' | 'editor';
  focusedSessionId: string | null;
  setView: (v: AppStore['view']) => void;
  /** view を渡すと切り替えも同時に行う（設計書 §6.1 の「カードクリックでターミナルへ直行」） */
  focusSession: (sessionId: string, view?: AppStore['view']) => void;
}

export const createUiSlice: StateCreator<AppStore, [], [], UiSlice> = (set) => ({
  view: 'kanban',
  focusedSessionId: null,

  setView: (view) => set({ view }),

  focusSession: (sessionId, view) =>
    set(view ? { focusedSessionId: sessionId, view } : { focusedSessionId: sessionId }),
});
