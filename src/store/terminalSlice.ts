import type { StateCreator } from 'zustand';
import type { AppStore } from './index';
import type { KanbanStatus } from '../types/model';

/**
 * ターミナル画面のタブ順。実行中のものから見たいので in_progress を先頭にする。
 * 列内の順序は sessionOrder（sort_order 順）をそのまま使う。
 */
export const TAB_COLUMN_ORDER: KanbanStatus[] = ['in_progress', 'backlog', 'review', 'done'];

export function selectTerminalTabs(state: Pick<AppStore, 'sessionOrder' | 'sessions'>): string[] {
  return TAB_COLUMN_ORDER.flatMap((status) => state.sessionOrder[status] ?? []).filter((id) => {
    const session = state.sessions[id];
    return session !== undefined && session.archived_at === null;
  });
}

export interface TerminalSlice {
  layout: 'single' | 'split2' | 'split2-v'; // 契約 §28.1。M1-3 は 'single' しか使わない
  paneAssignment: [string | null, string | null];
  activePane: 0 | 1;
  setLayout: (layout: TerminalSlice['layout']) => void;
  assignPane: (pane: 0 | 1, sessionId: string) => void;
  cycleSession: (dir: 1 | -1) => void;
}

export const createTerminalSlice: StateCreator<AppStore, [], [], TerminalSlice> = (set, get) => ({
  layout: 'single',
  paneAssignment: [null, null],
  activePane: 0,

  setLayout: (layout) => set({ layout }),

  assignPane: (pane, sessionId) =>
    set((state) => {
      const paneAssignment: [string | null, string | null] = [...state.paneAssignment];
      paneAssignment[pane] = sessionId;
      return { paneAssignment, activePane: pane, focusedSessionId: sessionId };
    }),

  // Cmd+J / Cmd+K。アクティブなペインのセッションだけを回す（M3-2 でも同じ）
  cycleSession: (dir) => {
    const state = get();
    const tabs = selectTerminalTabs(state);
    if (tabs.length === 0) return;
    const current = state.paneAssignment[state.activePane];
    const index = current === null ? -1 : tabs.indexOf(current);
    const nextIndex =
      index === -1 ? (dir === 1 ? 0 : tabs.length - 1) : (index + dir + tabs.length) % tabs.length;
    state.assignPane(state.activePane, tabs[nextIndex]);
  },
});
