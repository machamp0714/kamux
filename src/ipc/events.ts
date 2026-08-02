import { listen, type UnlistenFn } from '@tauri-apps/api/event';

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
