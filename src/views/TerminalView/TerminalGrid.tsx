import { useEffect, useLayoutEffect, useRef } from 'react';

import { useAppStore } from '../../store';
import {
  surfacesToDetach,
  visibleAgentSurfaces,
  visiblePanes,
  type PaneState,
} from '../../store/paneLogic';
import { isStarted } from '../../terminal/ptyBridge';
import { attachTerminal, detachTerminal, ensureTerminal } from '../../terminal/registry';
import { surfaceId } from '../../types/model';
import { syncPaneSize } from './paneSize';
import { TerminalPane } from './TerminalPane';
import { useActivePaneFocus } from './useActivePaneFocus';
import './TerminalGrid.css';

/** リサイズ通知のデバウンス。ドラッグ中の resize_pty 連打を抑える */
const RESIZE_DEBOUNCE_MS = 60;

/**
 * 1〜2 面のペイングリッド（契約 §28.4 / §57.3）。
 *
 * 責務は「グリッドの DOM」「表示集合の差分による attach/detach」「terminal 面の
 * DOM フォーカス（`useActivePaneFocus`）」の 3 つで、PTY のライフサイクル
 * （起動・購読・実行状態の seed）は各スロット内の `TerminalPane`
 * （DOM を描かない層）が持つ（契約 §85.5）。
 */
export function TerminalGrid(): JSX.Element {
  // terminal 面の唯一のフォーカス層（契約 §85.3 / §85.6）。DOM フォーカスは
  // activePane / modal の変化に追従してここから当たる
  useActivePaneFocus();

  const layout = useAppStore((s) => s.layout);
  const paneAssignment = useAppStore((s) => s.paneAssignment);
  const activePane = useAppStore((s) => s.activePane);
  const setActivePane = useAppStore((s) => s.setActivePane);

  /** ペイン index → attachTerminal に渡す DOM コンテナ。 */
  const hosts = useRef<(HTMLDivElement | null)[]>([null, null]);
  /** 前回の描画で attach 済みだったサーフェス集合。Zustand には入れない（契約 §10）。 */
  const attached = useRef<string[]>([]);

  const state: PaneState = { layout, paneAssignment, activePane };
  const panes = visiblePanes(state);

  /** ResizeObserver のコールバックから最新の割当を読むための箱。 */
  const stateRef = useRef<PaneState>(state);
  stateRef.current = state;

  // 表示集合の差分で attach/detach を駆動する（設計 §3.9）。
  // disposeTerminal は絶対に呼ばない（契約 §16）。
  //
  // useEffect ではなく useLayoutEffect であることには意味がある。React は 1 コミット内で
  // 「layout effect（子 → 親）→ passive effect（子 → 親）」の順に走らせるため、
  // 親であるこのグリッドの layout effect は、スロット内の TerminalPane（子）の
  // passive effect より必ず先に完了する。attach より前に fitTerminal / focus が
  // 走ると、fitTerminal は !attached で null を返して寸法が同期されないまま残る。
  // useEffect に「戻す」と、その順序保証が消える。
  useLayoutEffect(() => {
    const s: PaneState = { layout, paneAssignment, activePane };
    const next = visibleAgentSurfaces(s);

    for (const sid of surfacesToDetach(attached.current, next)) {
      detachTerminal(sid);
    }

    for (const pane of visiblePanes(s)) {
      const sessionId = s.paneAssignment[pane];
      const host = hosts.current[pane];
      if (sessionId === null || host === null) continue;
      const sid = surfaceId(sessionId, 'agent');
      ensureTerminal(sid);
      attachTerminal(sid, host);
      // Important 1: 未起動の PTY に resize_pty を投げても必ず NotFound で失敗する。
      // 再 attach 経路（画面外にいる間にウィンドウがリサイズされた場合など）では
      // 既に start_session を投げ済みの PTY に対して正しく効く必要があるので、
      // 単純に消してはならない。門は「起動済み」ではなく「start_session を投げ済み」の意味
      if (isStarted(sid)) {
        syncPaneSize(sid);
      }
    }

    attached.current = next;
  }, [layout, paneAssignment, activePane]);

  // Task 10 で usePaneFit が入ると、この ResizeObserver と usePaneFit の window.resize
  // 経路が二重になる。どちらを残すかは Task 10 で決めること。契約 §28 は「usePaneFit の
  // useEffect deps から layout を外す最適化」を禁止している —— 外すと向き変更時に xterm が
  // 横向きの cols/rows を保持したまま resize_pty に誤ったジオメトリが飛ぶ。表示は崩れないので
  // 発覚が遅れる。
  //
  // ResizeObserver はサイズが変化したときにしか発火しない（ポーリングではない）。
  // 表示中のペインが入れ替わる（= ホスト要素が増減する）のは layout / activePane の
  // 変化時なので、その 2 つで張り直す。割当だけの変化ではホスト要素は変わらない
  useEffect(() => {
    let timer: number | null = null;
    const observer = new ResizeObserver(() => {
      if (timer !== null) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        timer = null;
        // pty://exit の後もこのグリッドはマウントされたまま残りうる。exit 後に
        // ウィンドウ / コンテナがリサイズされると、存在しない PTY へ resize_pty を
        // 投げてしまう。attach 直後の呼び出しと同じ isStarted の門をここにも通す
        for (const sid of visibleAgentSurfaces(stateRef.current)) {
          if (isStarted(sid)) {
            syncPaneSize(sid);
          }
        }
      }, RESIZE_DEBOUNCE_MS);
    });

    for (const host of hosts.current) {
      if (host !== null) observer.observe(host);
    }

    return () => {
      if (timer !== null) window.clearTimeout(timer);
      observer.disconnect();
    };
  }, [layout, activePane]);

  // アンマウント時は全件 detach する（インスタンスは保持したまま）。
  useEffect(
    () => () => {
      for (const sid of surfacesToDetach(attached.current, [])) {
        detachTerminal(sid);
      }
      attached.current = [];
    },
    [],
  );

  return (
    <div className="terminal-grid" data-layout={layout}>
      {panes.map((pane) => {
        const sessionId = paneAssignment[pane];
        return (
          <div
            key={pane}
            className="terminal-pane-slot"
            data-pane={pane}
            data-active={pane === activePane}
            onFocusCapture={() => setActivePane(pane)}
            onMouseDown={() => setActivePane(pane)}
          >
            {sessionId === null ? (
              <div className="terminal-pane-slot__empty">
                左のタブからセッションを選択してください
              </div>
            ) : (
              <TerminalPane sessionId={sessionId} />
            )}
            <div
              className="terminal-pane-slot__host"
              ref={(el) => {
                hosts.current[pane] = el;
              }}
            />
          </div>
        );
      })}
    </div>
  );
}
