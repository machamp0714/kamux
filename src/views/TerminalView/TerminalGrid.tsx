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
import { usePaneFit } from './fitScheduler';
import { syncPaneSize } from './paneSize';
import { TerminalPane } from './TerminalPane';
import { useActivePaneFocus } from './useActivePaneFocus';
import './TerminalGrid.css';

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

  // レイアウト・割当・ウィンドウリサイズを rAF で畳み込んで fit → resize_pty する
  // （契約 §28 の反映、Task 10）。下の ResizeObserver（ホスト寸法の変化）も同じ
  // scheduler.request() へ合流させる —— 2 つの独立した debounce がそれぞれ
  // resize_pty を送ると、片方がリークしたときに重複送信になる（Task 8/9 の実測）。
  const fitScheduler = usePaneFit();

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

  // ホストのサイズが変化したら usePaneFit の scheduler へ委ねる（Task 10）。
  // 実際の isStarted 門・fitTerminal・resize_pty 呼び出しは fitScheduler.ts の
  // flush 側に閉じているので、ここでは request() を呼ぶだけでよい。request() は
  // 同一フレーム内なら何度呼んでも rAF が 1 本しか積まれないため、ここが仮に
  // 複数回発火しても resize_pty の重複送信にはならない。
  //
  // ResizeObserver はサイズが変化したときにしか発火しない（ポーリングではない）。
  // 表示中のペインが入れ替わる（= ホスト要素が増減する）のは layout / activePane の
  // 変化時なので、その 2 つで張り直す。割当だけの変化ではホスト要素は変わらない
  useEffect(() => {
    const observer = new ResizeObserver(() => {
      fitScheduler.request();
    });

    for (const host of hosts.current) {
      if (host !== null) observer.observe(host);
    }

    return () => {
      observer.disconnect();
    };
  }, [layout, activePane, fitScheduler]);

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
