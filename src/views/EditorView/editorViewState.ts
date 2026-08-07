import type { EditorSurfaceStatus } from '../../store/uiSlice';

export type EditorViewState =
  | { kind: 'no_session' }
  | { kind: 'starting' }
  | { kind: 'live' }
  | { kind: 'exited'; exitCode: number | null }
  | { kind: 'error'; message: string };

export function deriveEditorViewState(
  focusedSessionId: string | null,
  status: EditorSurfaceStatus | undefined,
): EditorViewState {
  if (focusedSessionId === null) return { kind: 'no_session' };
  if (status === undefined) return { kind: 'starting' };
  switch (status.kind) {
    case 'spawning':
      return { kind: 'starting' };
    case 'live':
      return { kind: 'live' };
    case 'exited':
      return { kind: 'exited', exitCode: status.exitCode };
    case 'error':
      return { kind: 'error', message: status.message };
  }
}

/** spawn_editor が返す上限エラー（AppError code = invalid_state）かどうか。 */
export function isEditorLimitError(message: string): boolean {
  return message.includes('editor limit reached');
}
