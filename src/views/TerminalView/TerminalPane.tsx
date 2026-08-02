import { useEffect, useRef } from 'react';

import { resizePty, startSession } from '../../ipc/commands';
import { useAppStore } from '../../store';
import {
  ensurePtySubscription,
  isStarted,
  markStarted,
  unmarkStarted,
} from '../../terminal/ptyBridge';
import {
  attachTerminal,
  detachTerminal,
  fitTerminal,
  getTerminal,
  invalidateFitCache,
  writeNotice,
} from '../../terminal/registry';
import { toAppError } from '../../store/uiSlice';
import { surfaceId } from '../../types/model';
import './TerminalView.css';

/** リサイズ通知のデバウンス。ドラッグ中の resize_pty 連打を抑える */
const RESIZE_DEBOUNCE_MS = 60;

export function TerminalPane({ sessionId }: { sessionId: string | null }): JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  // 契約 §16: フォーカスは呼び出し側の責務であり、modal === null のときにのみ当てる。
  // 「マウント時に 1 度評価する」ではなく modal の遷移に追従する必要があるため、
  // ここで購読してフォーカス専用 effect の依存にする（下の第 2 effect）
  const modal = useAppStore((s) => s.modal);

  useEffect(() => {
    const container = containerRef.current;
    if (container === null || sessionId === null) return undefined;
    const surface = surfaceId(sessionId, 'agent');

    attachTerminal(surface, container);

    // Important 1（PR 10 fix round 1）: 未起動の PTY に resize_pty を投げても
    // 必ず NotFound で失敗する（唯一の副作用である lastSize の更新も、この直後の
    // 起動成功パスで invalidateFitCache が捨てる）。isStarted の間だけ実行する
    // ——再 attach 経路（画面外にいる間にウィンドウがリサイズされた場合など）では
    // 既に起動済みの PTY に対して正しく効く必要があるので、単純に消してはならない。
    if (isStarted(surface)) {
      syncSize(surface);
    }

    // listen 登録の完了を待ってから起動する。待たないと最初のプロンプトを載せた
    // pty://data がリスナ不在で捨てられ、間欠的に「プロンプトが出ない」
    void ensurePtySubscription(surface)
      .then(() => {
        // pty://exit で起動済みフラグが落ちるので、切り替えて戻ると再起動される
        if (isStarted(surface)) return undefined;
        markStarted(surface);
        return startSession(sessionId).then(
          () => {
            // 必達 1（契約 §16 registry.ts）: 再起動された PTY は fitTerminal の
            // 直近サイズキャッシュにより resize_pty が飛ばず 80x24 のままになる。
            // キャッシュを無効化してから寸法を取り直す。
            invalidateFitCache(surface);
            syncSize(surface);
          },
          (error: unknown) => {
            // spawn 失敗では pty://exit が来ないので、ここで戻さないと再試行できない
            unmarkStarted(surface);
            const appError = toAppError(error);
            writeNotice(
              surface,
              `起動に失敗しました (${appError.code}): ${appError.message}`,
              'error',
            );
          },
        );
      })
      .catch((error: unknown) => {
        writeNotice(surface, `PTY イベントの購読に失敗しました: ${String(error)}`, 'error');
      });

    // ResizeObserver はサイズが変化したときにしか発火しない（ポーリングではない）
    let timer: number | null = null;
    const observer = new ResizeObserver(() => {
      if (timer !== null) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        timer = null;
        syncSize(surface);
      }, RESIZE_DEBOUNCE_MS);
    });
    observer.observe(container);

    return () => {
      if (timer !== null) window.clearTimeout(timer);
      observer.disconnect();
      detachTerminal(surface);
    };
  }, [sessionId]);

  // Critical（PR 10 fix round 1・契約 §16）: attachTerminal はもう term.focus() しない。
  // フォーカスは呼び出し側の責務であり、modal === null のときにのみ当てる。
  // モーダル表示中は実シェルへ DOM フォーカスを渡さない（Cmd+2 やタブクリックで
  // TerminalPane が（再）マウントされても、モーダルは view 分岐の外にあるため
  // 残ったままになりうる）。
  //
  // 上の attach effect とは別 effect にしているのは、「マウント時に 1 度評価する」
  // のではなく modal の遷移そのものに追従させるため——モーダルを閉じた瞬間
  // （modal: 非null → null）にも、この pane が再マウントされることなくフォーカスを
  // 戻す必要がある。attach effect の依存に modal を混ぜると、モーダルの開閉のたびに
  // detachTerminal → attachTerminal が走ってスクロールバックの扱いが不安定になる
  // （契約 §16 は detach/attach をペイン再割当専用の操作として定めている）。
  useEffect(() => {
    if (sessionId === null || modal !== null) return;
    getTerminal(surfaceId(sessionId, 'agent'))?.focus();
  }, [sessionId, modal]);

  // 契約 §57.3: attachTerminal のコンテナは常に .terminal-pane-slot__host である。
  // 空表示はスロット内のオーバーレイにする（コンテナと同じ箱を奪い合わせない）。
  // M3-2 の TerminalGrid はこの 2 要素構造をそのまま各ペインへ複製する
  return (
    <div className="terminal-pane-slot">
      {sessionId === null && (
        <div className="terminal-pane-slot__empty">左のタブからセッションを選択してください</div>
      )}
      <div className="terminal-pane-slot__host" ref={containerRef} />
    </div>
  );
}

/** fitTerminal は変化があったときだけ寸法を返す（契約 §16）。resize_pty はここが呼ぶ */
function syncSize(surface: string): void {
  const size = fitTerminal(surface);
  if (size === null) return;
  void resizePty(surface, size.cols, size.rows);
}
