import type { StateCreator } from 'zustand';
import type { AppStore } from './index';
import type { KanbanStatus, Layout } from '../types/model';
import {
  assignPaneReducer,
  cycleSessionReducer,
  setActivePaneReducer,
  setLayoutReducer,
  type PaneAssignment,
  type PaneIndex,
  type PaneState,
} from './paneLogic';

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
  layout: Layout;
  paneAssignment: PaneAssignment;
  activePane: PaneIndex;
  setLayout: (l: Layout) => void;
  assignPane: (pane: PaneIndex, sessionId: string) => void;
  setActivePane: (pane: PaneIndex) => void;
  cycleSession: (dir: 1 | -1) => void;
  resetTerminalLayout: () => void;
}

const INITIAL: PaneState = {
  layout: 'single',
  paneAssignment: [null, null],
  activePane: 0,
};

/**
 * 不変条件（契約 §85.1）: focusedSessionId === paneAssignment[activePane]。
 * ペイン系のアクションはすべてこれを通して focusedSessionId を書き戻す。
 *
 * 4 フィールドを明示的に射影しており、`{ ...p, focusedSessionId }` の spread に
 * 「簡略化」してはならない。reducer は変化が無いとき入力オブジェクトをそのまま
 * 返す（setLayoutReducer / setActivePaneReducer / cycleSessionReducer の早期 return）。
 * 呼び出し側は get() すなわち AppStore 全体を渡すため、spread にすると
 * ストア全体が set() に流れ込み、他スライスの状態を巻き戻す事故になる。
 */
const withFocus = (p: PaneState) => ({
  layout: p.layout,
  paneAssignment: p.paneAssignment,
  activePane: p.activePane,
  focusedSessionId: p.paneAssignment[p.activePane],
});

export const createTerminalSlice: StateCreator<AppStore, [], [], TerminalSlice> = (set, get) => ({
  ...INITIAL,

  setLayout: (l) => set(withFocus(setLayoutReducer(get(), l))),

  assignPane: (pane, sessionId) => set(withFocus(assignPaneReducer(get(), pane, sessionId))),

  setActivePane: (pane) => set(withFocus(setActivePaneReducer(get(), pane))),

  cycleSession: (dir) => set(withFocus(cycleSessionReducer(get(), selectTerminalTabs(get()), dir))),

  resetTerminalLayout: () => set({ ...INITIAL, focusedSessionId: null }),
});
