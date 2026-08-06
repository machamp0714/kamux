/**
 * 契約 §11 のキーマップ。M1-2 が実装したのは Cmd+1 / Cmd+N と、モーダルを閉じる Escape。
 * M1-3 は Cmd+2 と Cmd+J / Cmd+K を追加する。M3-1 は Cmd+3 を追加する。
 * Cmd+D, Cmd+[, Cmd+]（M3-2）/ Cmd+P（M3-4）は
 * 後続フェーズでこのユニオンに variant を足す形で追加する。
 */
export type KeymapAction =
  | { type: 'set_view'; view: 'kanban' | 'terminal' | 'editor' }
  | { type: 'open_create_session' }
  | { type: 'close_modal' }
  | { type: 'cycle_session'; dir: 1 | -1 };

export interface KeymapEvent {
  key: string;
  metaKey: boolean;
}

export interface KeymapContext {
  modalOpen: boolean;
  /** Cmd+J / Cmd+K は terminal 画面でのみ有効（契約 §11） */
  view: 'kanban' | 'terminal' | 'editor';
}

export function resolveKeymap(e: KeymapEvent, ctx: KeymapContext): KeymapAction | null {
  if (e.metaKey) {
    if (e.key === '1') return { type: 'set_view', view: 'kanban' };
    if (e.key === '2') return { type: 'set_view', view: 'terminal' };
    if (e.key === '3') return { type: 'set_view', view: 'editor' };
    if (e.key === 'n' || e.key === 'N') return { type: 'open_create_session' };
    if (e.key === 'j' || e.key === 'J') {
      return ctx.view === 'terminal' ? { type: 'cycle_session', dir: 1 } : null;
    }
    if (e.key === 'k' || e.key === 'K') {
      return ctx.view === 'terminal' ? { type: 'cycle_session', dir: -1 } : null;
    }
    return null;
  }
  if (e.key === 'Escape' && ctx.modalOpen) return { type: 'close_modal' };
  return null;
}
