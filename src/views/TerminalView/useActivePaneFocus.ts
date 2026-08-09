import { useEffect } from 'react';
import { useAppStore } from '../../store';
import type { PaneState } from '../../store/paneLogic';
import { surfaceId } from '../../types/model';
import { getTerminal } from '../../terminal/registry';

/** DOM フォーカスを持つべき agent サーフェス。無ければ null。 */
export function focusTargetSurfaceId(s: PaneState): string | null {
  const sessionId = s.paneAssignment[s.activePane];
  return sessionId === null ? null : surfaceId(sessionId, 'agent');
}

/**
 * activePane state を正として DOM フォーカスを追従させる（設計 §3.1、terminal 面の唯一の
 * フォーカス層。契約 §85.3 / §85.6）。
 * 逆方向（DOM → state）は TerminalGrid の onFocusCapture / onMouseDown が担う。
 * 双方向のループは「既に目標が合焦していれば focus() を呼ばない」で断つ。
 *
 * 契約 §85.4: `modal` はガードと依存配列の両方に持つ。片方だけでは足りない
 * ——ガードだけだとモーダルを閉じた後に無フォーカスのまま残り、依存だけだと
 * モーダル表示中に実 $SHELL の PTY へ打鍵が流れる（別々の穴、別々の変異）。
 */
export function useActivePaneFocus(): void {
  const view = useAppStore((s) => s.view);
  const modal = useAppStore((s) => s.modal);
  const layout = useAppStore((s) => s.layout);
  const paneAssignment = useAppStore((s) => s.paneAssignment);
  const activePane = useAppStore((s) => s.activePane);

  useEffect(() => {
    if (view !== 'terminal' || modal !== null) return;

    const target = focusTargetSurfaceId({ layout, paneAssignment, activePane });
    if (target === null) return;

    // 契約 §16 の getTerminal は Terminal | undefined。まだ生成されていない
    // タイミング（TerminalGrid の attach effect より前）を素通しする。
    const term = getTerminal(target);
    if (term === undefined) return;
    if (term.textarea !== null && document.activeElement === term.textarea) return;

    term.focus();
  }, [view, modal, layout, paneAssignment, activePane]);
}
