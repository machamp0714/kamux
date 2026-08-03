import { decodeBase64 } from '../ipc/base64';
import { onPtyData, onPtyExit } from '../ipc/events';
import { ensureTerminal, writeNotice, writeToTerminal } from './registry';

interface Subscription {
  ready: Promise<void>;
  unlisten: Array<() => void>;
  /** ready が pending のうちに dispose された場合、解決した端から即座に unlisten する */
  disposed: boolean;
}

const subscriptions = new Map<string, Subscription>();
/** start_session を投げ済みのサーフェス。pty://exit で消す */
const startedSurfaces = new Set<string>();

export function isStarted(surfaceId: string): boolean {
  return startedSurfaces.has(surfaceId);
}

export function markStarted(surfaceId: string): void {
  startedSurfaces.add(surfaceId);
}

/**
 * 起動に失敗したときに呼ぶ。spawn 自体が失敗すると PTY が存在しないので
 * pty://exit は来ない。ここで戻さないと、そのタブは二度と起動を試みなくなる。
 */
export function unmarkStarted(surfaceId: string): void {
  startedSurfaces.delete(surfaceId);
}

/**
 * pty://data と pty://exit を購読する。返る Promise は listen 登録の完了で解決する。
 *
 * listen も invoke と同じ IPC 往復なので、登録完了前に start_session を投げると
 * シェルの最初のプロンプトを載せたイベントがリスナ不在で捨てられる（Tauri はバッファしない）。
 * 契約 §16 の attachTerminal は void 固定なので、待ち合わせはここが担う。
 *
 * `pty://exit` の後にも `pty://data` が最大 1 件飛びうる（Rust 側の bounded join の
 * Timeout 分岐）ため、exit ハンドラは購読の解除や dispose を一切行わない。
 * 世代が古い（`still_current == false`）exit は Rust 側で握りつぶされフロントへは
 * 転送されないので、ここで「exit 待ち」の中間状態を作る必要はない。
 */
export function ensurePtySubscription(surfaceId: string): Promise<void> {
  const existing = subscriptions.get(surfaceId);
  if (existing) return existing.ready;

  // イベントが来た瞬間に書き込めるよう、購読前にインスタンスを用意する
  ensureTerminal(surfaceId);

  const unlisten: Array<() => void> = [];
  const subscription: Subscription = { ready: Promise.resolve(), unlisten, disposed: false };

  // 各 listen を個別に .then でハンドルへ変換してから Promise.all する。
  // まとめて `Promise.all([onPtyData(...), onPtyExit(...)])` に `.then(handles => ...)` を
  // 付けるだけだと、一方が reject したときにもう一方の resolve 済みハンドルが
  // どこにも記録されず孤児になる（次の ensure で二重登録される = 文字が二重に出る）。
  const dataReady = onPtyData(surfaceId, ({ base64, seq }) => {
    writeToTerminal(surfaceId, decodeBase64(base64), seq);
  }).then((handle) => {
    registerUnlisten(subscription, handle);
  });

  const exitReady = onPtyExit(surfaceId, ({ exit_code: exitCode }) => {
    // 起動済みフラグを落とすことで、タブを切り替えて戻ると再起動される
    startedSurfaces.delete(surfaceId);
    writeNotice(surfaceId, `[process exited: ${exitCode ?? 'signal'}]`);
  }).then((handle) => {
    registerUnlisten(subscription, handle);
  });

  subscription.ready = Promise.all([dataReady, exitReady]).then(
    () => undefined,
    (error: unknown) => {
      // 登録に失敗したら再試行できるよう捨てる（残すと rejected promise が固定される）。
      // data/exit の片方だけ成功していた場合、そのハンドルは subscription.unlisten に
      // 積まれているが subscriptions map からは消えるので、ここで確実に解除しないと
      // 次の再試行時に listener が積み上がる（文字が二重に出る）。
      subscriptions.delete(surfaceId);
      subscription.unlisten.forEach((unlisten) => unlisten());
      subscription.unlisten.length = 0;
      throw error;
    },
  );

  subscriptions.set(surfaceId, subscription);
  return subscription.ready;
}

/** 解決した listen ハンドルを記録する。すでに dispose 済みなら即座に解除する */
function registerUnlisten(subscription: Subscription, handle: () => void): void {
  if (subscription.disposed) {
    handle();
    return;
  }
  subscription.unlisten.push(handle);
}

/**
 * 購読を解除する。`disposeTerminal` と対で呼ぶ。
 * 忘れるとサーフェスを開き直すたびにリスナが積み上がり、
 * 1 チャンクが N 回書き込まれて「文字が二重に出る」症状になる。
 *
 * `ensurePtySubscription` の Promise が解決する前に呼ばれても、
 * 後から解決したハンドルを取りこぼさず解除する（`disposed` フラグ経由）。
 */
export function disposePtySubscription(surfaceId: string): void {
  const subscription = subscriptions.get(surfaceId);
  if (!subscription) return;
  subscription.disposed = true;
  subscription.unlisten.forEach((unlisten) => unlisten());
  subscriptions.delete(surfaceId);
  startedSurfaces.delete(surfaceId);
}

/** テスト専用。モジュールスコープの状態を初期化する */
export function resetPtyBridgeForTest(): void {
  subscriptions.clear();
  startedSurfaces.clear();
}
