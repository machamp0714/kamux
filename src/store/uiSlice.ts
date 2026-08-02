import type { StateCreator } from 'zustand';

import type { AppError } from '../types/model';
import type { AppStore } from './index';

/** 開いているモーダルの種類。M3-4 の Cmd+P で variant を追加する。 */
export type ModalState = { kind: 'create_session' } | { kind: 'edit_session'; sessionId: string };

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
  /** view を渡すと切り替えも同時に行う（設計書 §6.1 の「カードクリックでターミナルへ直行」） */
  focusSession: (sessionId: string, view?: AppStore['view']) => void;
  modal: ModalState | null;
  /** 副作用: setView('kanban')。Cmd+N はターミナル画面からも効く（契約 §11）。 */
  openModal: (m: ModalState) => void;
  closeModal: () => void;
  lastError: AppError | null;
  setError: (e: AppError | null) => void;
}

export const createUiSlice: StateCreator<AppStore, [], [], UiSlice> = (set) => ({
  view: 'kanban',
  focusedSessionId: null,

  setView: (view) => set({ view }),

  focusSession: (sessionId, view) =>
    set(view ? { focusedSessionId: sessionId, view } : { focusedSessionId: sessionId }),

  modal: null,
  openModal: (m) => set({ modal: m, view: 'kanban' }),
  closeModal: () => set({ modal: null }),
  lastError: null,
  setError: (e) => set({ lastError: e }),
});
