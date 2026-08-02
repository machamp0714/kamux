/**
 * 契約 §11 のキーマップ。M1-2 が実装するのは Cmd+1 / Cmd+N と、モーダルを閉じる Escape。
 * Cmd+2 と Cmd+J,K（M1-3）/ Cmd+3（M3-1）/ Cmd+D, Cmd+[, Cmd+]（M3-2）/
 * Cmd+P（M3-4）は後続フェーズでこのユニオンに variant を足す形で追加する。
 */
export type KeymapAction =
  { type: 'set_view'; view: 'kanban' } | { type: 'open_create_session' } | { type: 'close_modal' };

export interface KeymapEvent {
  key: string;
  metaKey: boolean;
}

export interface KeymapContext {
  modalOpen: boolean;
}

export function resolveKeymap(e: KeymapEvent, ctx: KeymapContext): KeymapAction | null {
  if (e.metaKey) {
    if (e.key === '1') return { type: 'set_view', view: 'kanban' };
    if (e.key === 'n' || e.key === 'N') return { type: 'open_create_session' };
    return null;
  }
  if (e.key === 'Escape' && ctx.modalOpen) return { type: 'close_modal' };
  return null;
}
