import { useEffect } from 'react';

import { startSession } from '../../ipc/commands';
import {
  ensurePtySubscription,
  isStarted,
  markStarted,
  unmarkStarted,
} from '../../terminal/ptyBridge';
import { invalidateFitCache, writeNotice } from '../../terminal/registry';
import { useAppStore } from '../../store';
import { toAppError } from '../../store/uiSlice';
import { surfaceId } from '../../types/model';
import { syncPaneSize } from './paneSize';

/**
 * 1 ペインぶんの PTY ライフサイクル層（契約 §85.5）。**DOM を描かない。**
 *
 * グリッドの DOM と attach/detach は `TerminalGrid` が持ち、この層は
 * 「そのペインに載っているセッションのプロセスを立ち上げて維持する」ことだけを持つ:
 * PTY イベントの購読 → 二重起動の門 → start_session → 実行状態の seed / 失敗の記録。
 *
 * `TerminalGrid` は各スロットの中でこれを 1 つずつ描くので、2 ペインでは 2 つ生きる。
 * 門（`isStarted`）は surface 単位（`{session_id}:agent`）なので、スワップで同じ
 * セッションが別のペインへ載り替えても start_session は 1 回のままである。
 *
 * **フォーカスはここでは当てない。** terminal 面の DOM フォーカスは
 * `useActivePaneFocus` の 1 層に統合されている（契約 §85.3 / §85.6）。
 */
export function TerminalPane({ sessionId }: { sessionId: string }): null {
  useEffect(() => {
    const surface = surfaceId(sessionId, 'agent');

    // listen 登録の完了を待ってから起動する。待たないと最初のプロンプトを載せた
    // pty://data がリスナ不在で捨てられ、間欠的に「プロンプトが出ない」
    void ensurePtySubscription(surface)
      .then(() => {
        // 契約 §85.5 条件 1: 二重起動の門。ここでの isStarted は「起動済み（spawn 完了）」
        // ではなく「start_session を投げ済み」の意味である——markStarted は
        // startSession() の解決を待たずに呼ばれる。pty://exit で起動済みフラグが
        // 落ちるので、切り替えて戻ると再起動される。
        // **2 ペインでも消してはならない**——スワップで同じセッションのこの層が
        // もう一方のペインで再実行されると、門が無ければ二重に spawn される
        if (isStarted(surface)) return undefined;
        markStarted(surface);
        return startSession(sessionId).then(
          (session) => {
            // 計画 §4.10: 戻り値の Session には consumer が DB を更新済みの
            // last_runtime_state が入っている。イベント（session://state/{session_id}）を
            // 取りこぼしても、コマンドの戻り値で表示が自己修復する。
            // **この非 reset 経路には first_started_at の除外を適用しない**（契約 §34.6）
            // —— 戻り値は mark_first_started の非同期コミットより前に読まれて
            // first_started_at === null を持ちうる。除外は seedRuntimeStates 側の
            // reset 経路にだけある。
            useAppStore.getState().seedRuntimeStates([session]);
            // 必達 1（契約 §16 registry.ts）: 再起動された PTY は fitTerminal の
            // 直近サイズキャッシュにより resize_pty が飛ばず 80x24 のままになる。
            // キャッシュを無効化してから寸法を取り直す。
            invalidateFitCache(surface);
            syncPaneSize(surface);
          },
          (error: unknown) => {
            // spawn 失敗では pty://exit が来ないので、ここで戻さないと再試行できない
            unmarkStarted(surface);
            const appError = toAppError(error);
            // 契約 §42.3 規約 4: mark_error が DB へ書くのと同一の文字列をストアにも残す。
            // カードの kanban-card__error はこれを読む。**許可リスト（契約 §40.3）は
            // 複製しない** —— ズレの境界は契約 §42.3.1 が定めており、許可リストを
            // 2 箇所に散らして片方だけ更新されるドリフトの方が高くつく。
            useAppStore.getState().setRuntimeError(sessionId, appError.message);
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
  }, [sessionId]);

  return null;
}
