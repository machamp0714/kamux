import type { PaneIndex } from '../store/paneLogic';

/**
 * 契約 §11 のキーマップ。M1-2 が実装したのは Cmd+1 / Cmd+N と、モーダルを閉じる Escape。
 * M1-3 は Cmd+2 と Cmd+J / Cmd+K を追加した。M3-1 は Cmd+3 を追加した。
 * M3-2 が Cmd+D / Cmd+[ / Cmd+]（と WKWebView 奪取時のフォールバック Cmd+Alt+←/→）を追加する。
 * Cmd+P（M3-4）は後続フェーズでこのユニオンに variant を足す形で追加する。
 *
 * 契約 §11.3-4「キー → アクションの対応表の正典は §11 であり、その実装は同時に
 * 1 つだけ存在してよい」により、terminal 画面固有のキーも独立した解決関数へ
 * 切り出さず、このユニオンと resolveKeymap 本体に足す。
 */
export type KeymapAction =
  | { type: 'set_view'; view: 'kanban' | 'terminal' | 'editor' }
  | { type: 'open_create_session' }
  | { type: 'close_modal' }
  | { type: 'cycle_session'; dir: 1 | -1 }
  | { type: 'set_active_pane'; pane: PaneIndex }
  | { type: 'toggle_layout' };

export interface KeymapEvent {
  key: string;
  metaKey: boolean;
  /** Cmd+J / Cmd+K / Cmd+D / Cmd+[ / Cmd+] の判定に使う（Ctrl 併用時はアプリが消費しない。契約 §97.2 規則 C） */
  ctrlKey: boolean;
  /** Cmd+Alt+←/→（Cmd+[/] のフォールバック）の判定に使う */
  altKey: boolean;
}

export interface KeymapContext {
  modalOpen: boolean;
  /** Cmd+J / Cmd+K / Cmd+D / Cmd+[ / Cmd+] は terminal 画面でのみ有効（契約 §11.4.2） */
  view: 'kanban' | 'terminal' | 'editor';
}

/**
 * Cmd+[ / Cmd+] の解決表。フォールバックへ切り替える必要が生じた場合、この 1 箇所の
 * キーを差し替えるだけでよい（人間ゲートの判定 = 契約 §11「WebView にキーを奪われる
 * 可能性があるため実機確認」）。Cmd+Alt+←/→ は WKWebView 奪取時のフォールバックとして
 * 最初から同時に実装しており、こちらは表を変えずに残る。
 */
const PANE_KEY_TABLE: Record<string, PaneIndex> = {
  '[': 0,
  ']': 1,
};

function resolveTerminalOnlyAction(e: KeymapEvent): KeymapAction | null {
  if (e.ctrlKey) return null;

  if (e.altKey) {
    if (e.key === 'ArrowLeft') return { type: 'set_active_pane', pane: 0 };
    if (e.key === 'ArrowRight') return { type: 'set_active_pane', pane: 1 };
    return null;
  }

  if (e.key === 'd' || e.key === 'D') return { type: 'toggle_layout' };
  if (e.key in PANE_KEY_TABLE) {
    return { type: 'set_active_pane', pane: PANE_KEY_TABLE[e.key] };
  }
  return null;
}

export function resolveKeymap(e: KeymapEvent, ctx: KeymapContext): KeymapAction | null {
  if (e.metaKey) {
    if (e.key === '1') return { type: 'set_view', view: 'kanban' };
    if (e.key === '2') return { type: 'set_view', view: 'terminal' };
    if (e.key === '3') return { type: 'set_view', view: 'editor' };
    if (e.key === 'n' || e.key === 'N') return { type: 'open_create_session' };
    if (e.key === 'j' || e.key === 'J') {
      return ctx.view === 'terminal' && !e.ctrlKey ? { type: 'cycle_session', dir: 1 } : null;
    }
    if (e.key === 'k' || e.key === 'K') {
      return ctx.view === 'terminal' && !e.ctrlKey ? { type: 'cycle_session', dir: -1 } : null;
    }
    return ctx.view === 'terminal' ? resolveTerminalOnlyAction(e) : null;
  }
  if (e.key === 'Escape' && ctx.modalOpen) return { type: 'close_modal' };
  return null;
}
