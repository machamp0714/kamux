import { resizePty } from '../../ipc/commands';
import { fitTerminal } from '../../terminal/registry';

/**
 * 表示中のサーフェスを fit して、寸法が変わっていれば PTY に伝える。
 *
 * `fitTerminal` は変化があったときだけ寸法を返す（契約 §16）。`resize_pty` を
 * 呼ぶのは呼び出し側の責務なので、その 1 対 1 の組をここに閉じ込める。
 *
 * **`isStarted` の門はここに入れない。** 門は呼び出し口ごとに意味が違う
 * （attach 直後 / ResizeObserver / start_session 解決後）ため、呼び出し側に置いて
 * 「どの経路の門か」がテストから見えるようにしてある（TerminalGrid / TerminalPane）。
 */
export function syncPaneSize(surface: string): void {
  const size = fitTerminal(surface);
  if (size === null) return;
  // Important 1（PR 10 fix round 2）: isStarted の門をすり抜ける in-flight の窓
  // （start_session を投げた直後、実際の spawn が完了する前）では resize_pty が
  // NotFound で reject されうる。ackPty / writePty と同じ理由（もう存在しない、
  // またはまだ存在しない PTY への通知なので、失敗しても何も壊れない）で握り潰す。
  resizePty(surface, size.cols, size.rows).catch(() => {
    // 上記の理由により意図的に無視する
  });
}
