import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import type { FocusPayload } from '../types/model';

export interface PtyDataPayload {
  base64: string;
  seq: number;
}

export interface PtyExitPayload {
  surface_id: string;
  exit_code: number | null;
}

export const onPtyData = (
  surfaceId: string,
  handler: (payload: PtyDataPayload) => void,
): Promise<UnlistenFn> =>
  listen<PtyDataPayload>(`pty://data/${surfaceId}`, (event) => handler(event.payload));

export const onPtyExit = (
  surfaceId: string,
  handler: (payload: PtyExitPayload) => void,
): Promise<UnlistenFn> =>
  listen<PtyExitPayload>(`pty://exit/${surfaceId}`, (event) => handler(event.payload));

/**
 * `focus://session/{session_id}`（契約 §8）の受信。
 * 同一トピックの listen ラッパの正典は本関数（契約 §35）。emit するのは M2-3（通知クリック）。
 * M1-4 は受け口のみ用意し、カードクリックと同じ focusSession（uiSlice）に収束させる。
 */
export function listenFocus(sessionId: string, cb: (p: FocusPayload) => void): Promise<UnlistenFn> {
  return listen<FocusPayload>(`focus://session/${sessionId}`, (e) => cb(e.payload));
}
