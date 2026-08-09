import type { StateCreator } from 'zustand';

import type { AppError } from '../types/model';
import type { AppStore } from './index';
import { routeFocusReducer } from './paneLogic';

/** 開いているモーダルの種類。M3-4 の Cmd+P で variant を追加する。 */
export type ModalState = { kind: 'create_session' } | { kind: 'edit_session'; sessionId: string };

/** nvim エディタサーフェスの状態（M3-1）。EditorView が spawn/live/exited/error を判別するために使う。 */
export type EditorSurfaceStatus =
  | { kind: 'spawning' }
  | { kind: 'live' }
  | { kind: 'exited'; exitCode: number | null }
  | { kind: 'error'; message: string };

function sameStatus(a: EditorSurfaceStatus | undefined, b: EditorSurfaceStatus): boolean {
  if (a === undefined || a.kind !== b.kind) return false;
  if (a.kind === 'exited' && b.kind === 'exited') return a.exitCode === b.exitCode;
  if (a.kind === 'error' && b.kind === 'error') return a.message === b.message;
  return true;
}

/** status が null ならそのセッションのエントリを削除する。変化が無ければ同じ参照を返す。 */
export function reduceEditorSurfaces(
  current: Record<string, EditorSurfaceStatus>,
  sessionId: string,
  status: EditorSurfaceStatus | null,
): Record<string, EditorSurfaceStatus> {
  if (status === null) {
    if (!(sessionId in current)) return current;
    const next = { ...current };
    delete next[sessionId];
    return next;
  }
  if (sameStatus(current[sessionId], status)) return current;
  return { ...current, [sessionId]: status };
}

/** invoke が投げた未知の値を AppError（契約 §6）へ正規化する。 */
export function toAppError(e: unknown): AppError {
  if (
    typeof e === 'object' &&
    e !== null &&
    'code' in e &&
    typeof (e as { code: unknown }).code === 'string' &&
    'message' in e &&
    typeof (e as { message: unknown }).message === 'string'
  ) {
    return e as AppError;
  }
  return { code: 'io', message: String(e) };
}

export interface UiSlice {
  view: 'kanban' | 'terminal' | 'editor';
  focusedSessionId: string | null;
  setView: (v: AppStore['view']) => void;
  /**
   * カードクリック / Enter / 通知クリック（M2-3）の共通の着地点（設計判断 10・要件5）。
   * アクティブペインへ該当セッションを割り当ててから画面を切り替える。
   * view を省略すると 'terminal' が既定（契約 §11）。
   */
  focusSession: (sessionId: string, view?: AppStore['view']) => void;
  modal: ModalState | null;
  /** 副作用: setView('kanban')。Cmd+N はターミナル画面からも効く（契約 §11）。 */
  openModal: (m: ModalState) => void;
  closeModal: () => void;
  lastError: AppError | null;
  setError: (e: AppError | null) => void;
  editorSurfaces: Record<string, EditorSurfaceStatus>;
  setEditorSurface: (sessionId: string, status: EditorSurfaceStatus | null) => void;
}

export const createUiSlice: StateCreator<AppStore, [], [], UiSlice> = (set) => ({
  view: 'kanban',
  focusedSessionId: null,

  setView: (view) => set({ view }),

  // routeFocusReducer（paneLogic.ts）へ委譲する。もう一方のペインに既に出ている
  // 場合は割当を動かさず activePane だけ移し、それ以外は assignPaneReducer 経由で
  // アクティブペインへ割り当てる（契約 §85.1 / §85.2）。
  focusSession: (sessionId, view = 'terminal') =>
    set((s) => {
      const routed = routeFocusReducer(s, sessionId);
      return {
        layout: routed.layout,
        paneAssignment: routed.paneAssignment,
        activePane: routed.activePane,
        focusedSessionId: sessionId,
        view,
      };
    }),

  modal: null,
  openModal: (m) => set({ modal: m, view: 'kanban' }),
  closeModal: () => set({ modal: null }),
  lastError: null,
  setError: (e) => set({ lastError: e }),

  editorSurfaces: {},
  setEditorSurface: (sessionId, status) =>
    set((s) => ({ editorSurfaces: reduceEditorSurfaces(s.editorSurfaces, sessionId, status) })),
});
