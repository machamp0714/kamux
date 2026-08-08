import { useEffect, useMemo, useRef } from 'react';

import { useAppStore } from '../../store';
import { visibleAgentSurfaces, type PaneState } from '../../store/paneLogic';
import { isStarted } from '../../terminal/ptyBridge';
import { syncPaneSize } from './paneSize';

export interface FitScheduler {
  request(): void;
  cancel(): void;
}

/**
 * 同一フレーム内の複数の fit 要求を 1 回に畳む。
 * 要求が無ければ rAF を 1 度も積まないので、契約 §0「アイドル CPU ほぼ 0%」を満たす。
 * setInterval / 無条件デバウンスタイマーは使わない（ポーリング禁止）。
 */
export function createFitScheduler(
  flush: () => void,
  raf: (cb: () => void) => number = (cb) => window.requestAnimationFrame(cb),
  caf: (handle: number) => void = (handle) => window.cancelAnimationFrame(handle),
): FitScheduler {
  let handle: number | null = null;

  return {
    request() {
      if (handle !== null) return;
      handle = raf(() => {
        handle = null;
        flush();
      });
    },
    cancel() {
      if (handle === null) return;
      caf(handle);
      handle = null;
    },
  };
}

/**
 * レイアウト・割当・ウィンドウサイズの変化を検知して、表示中ペインだけを
 * fit → resize_pty する。TerminalGrid から呼ぶ。
 *
 * 戻り値の scheduler は TerminalGrid の ResizeObserver 経路とも共有する
 * （契約 §28 の反映 / M3-2 Task 8-9 の申し送り）。usePaneFit 自身の状態変化トリガと
 * ResizeObserver のホスト寸法トリガを同じ rAF 畳み込みへ合流させることで、
 * 2 つの独立した debounce がそれぞれ resize_pty を送る事故
 * （Task 8/9 実測: リークした ResizeObserver の数だけ resize_pty が重複した）を
 * 構造的に防ぐ。TerminalGrid は scheduler.request() を呼ぶだけで、実際の
 * isStarted 門と resize_pty 呼び出しはここに閉じる。
 */
export function usePaneFit(): FitScheduler {
  const view = useAppStore((s) => s.view);
  const layout = useAppStore((s) => s.layout);
  const paneAssignment = useAppStore((s) => s.paneAssignment);
  const activePane = useAppStore((s) => s.activePane);

  const stateRef = useRef<PaneState>({ layout, paneAssignment, activePane });
  stateRef.current = { layout, paneAssignment, activePane };

  const scheduler = useMemo(
    () =>
      createFitScheduler(() => {
        for (const sid of visibleAgentSurfaces(stateRef.current)) {
          // 未起動 / exited の PTY へ resize_pty を投げない。呼び出し口ごとに
          // 意味が違うため、門は呼び出し側に置く（paneSize.ts の設計コメント）。
          // ここは 3 つ目の呼び出し口（attach 直後の即時同期 / ResizeObserver /
          // ここ）であり、同じ門を通す。
          if (!isStarted(sid)) continue;
          syncPaneSize(sid);
        }
      }),
    [],
  );

  // deps に layout を直接含めること（契約 §28.2）。split2 ⇔ split2-v は
  // ペイン数が変わらないため、visiblePanes(s) や paneAssignment にキーイングすると
  // 向き変更で再 fit が走らず、xterm が横向きの cols/rows を保持したまま
  // resize_pty に誤ったジオメトリを送る。表示は崩れないので発覚が遅れる
  //
  // TerminalGrid.tsx の attach useLayoutEffect も [layout, paneAssignment, activePane]
  // を deps に持ち、多くの遷移ではここと冗長に同じ syncPaneSize を呼ぶ。ただし
  // 冗長なのは「両方が isStarted=true を見られる」場合だけである。single → split2 で
  // 新しく可視になるペインのように、親の attach effect が走った時点ではまだ
  // isStarted=false（PTY 起動要求がまだ解決していない）のことがある。その場合
  // attach effect は syncPaneSize をスキップし、子の passive effect で
  // isStarted が true になるのは次のフレームである。このフレーム差を埋めるのは
  // この layout dep だけであり、外すと新ペインが旧ジオメトリのまま固定される
  // （Task 10 レビュー Important 1、§89.1.1 で実測確認済み。
  // TerminalGrid.test.tsx の「single → split2 の直後に...」が固定する）
  useEffect(() => {
    if (view !== 'terminal') return;
    scheduler.request();
  }, [view, layout, paneAssignment, activePane, scheduler]);

  useEffect(() => {
    const onResize = () => scheduler.request();
    window.addEventListener('resize', onResize);
    return () => {
      window.removeEventListener('resize', onResize);
      scheduler.cancel();
    };
  }, [scheduler]);

  return scheduler;
}
