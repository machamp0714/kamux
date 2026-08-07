// 契約 §26.2: window.__TAURI_INTERNALS__.invoke を差し替える E2E 用の IPC モック。
// @tauri-apps/api/mocks の mockIPC は jsdom 前提のため使わない（公式ドキュメントが
// 実ブラウザ用途には @wdio/tauri-service を案内している）。

export interface TauriMockSpec {
  /** コマンド名 → 戻り値を返す関数。args は invoke の第 2 引数そのまま */
  commands: Record<string, (args: Record<string, unknown>) => unknown>;
}

/** move_session の発火を spec 側でアサートするための呼び出し履歴 1 件。 */
export interface TauriMockCall {
  cmd: string;
  args: unknown;
}

// window.__TAURI_INTERNALS__ には @tauri-apps/api 側の型定義が無いため、
// e2e (page.evaluate 越し) と本ファイルの両方が参照できるようここで宣言する。
// src/ 側はこのプロパティに触れないため、他のグローバル宣言と衝突しない。
declare global {
  interface Window {
    __TAURI_INTERNALS__: {
      invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
      transformCallback: (cb: unknown) => unknown;
      __kamuxCalls: TauriMockCall[];
      /** event 名 → @tauri-apps/api/event の listen() が渡すハンドラ配列（plugin:event|listen が積む） */
      __kamuxEventHandlers: Record<
        string,
        Array<(event: { event: string; payload: unknown }) => void>
      >;
      /** spec 側から合成 emit するためのヘルパ（page.evaluate 経由で呼ぶ） */
      __kamuxEmit: (event: string, payload: unknown) => void;
    };
  }
}

/**
 * window.__TAURI_INTERNALS__.invoke を差し替えるスクリプトを組み立てる。
 * page.addInitScript() はブラウザ側で関数を再評価するだけで、渡した関数のクロージャは
 * 持ち込めない。そのため tauriMockScript 自身は文字列を返す純関数にし、
 * spec.commands の各関数本体は toString() で文字列化して埋め込む
 * ——**渡す関数は外側の変数を参照しないこと**（参照するとブラウザ側で undefined になる）。
 *
 * `plugin:event|listen` / `plugin:event|unlisten` はコマンド名として spec.commands には
 * 書かせない。`@tauri-apps/api/event` の `listen()` は `invoke('plugin:event|listen', ...)`
 * を呼び、`handler` には `transformCallback(handler)` の戻り値が渡る（下で
 * `transformCallback` を恒等関数にしてあるので、渡ってくるのはハンドラ関数そのもの）。
 * これを `__kamuxEventHandlers[event]` に積み、`__kamuxEmit` で叩けるようにする。
 */
export function tauriMockScript(spec: TauriMockSpec): string {
  const entries = Object.entries(spec.commands)
    .map(([name, fn]) => `${JSON.stringify(name)}: (${fn.toString()})`)
    .join(',\n');
  return `
    window.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__ || {};
    window.__TAURI_INTERNALS__.__kamuxCalls = [];
    window.__TAURI_INTERNALS__.__kamuxEventHandlers = {};
    const handlers = { ${entries} };
    // listen() が返す id は下の 'plugin:event|listen' が返す配列長（= index + 1）である。
    // 解除しても配列は詰めない（詰めると既に配布済みの id が別のハンドラを指す）。
    const removeListener = (event, eventId) => {
      const list = window.__TAURI_INTERNALS__.__kamuxEventHandlers[event];
      if (!list || typeof eventId !== 'number') return;
      if (list[eventId - 1] !== undefined) list[eventId - 1] = () => {};
    };
    window.__TAURI_INTERNALS__.invoke = (cmd, args) => {
      window.__TAURI_INTERNALS__.__kamuxCalls.push({ cmd, args });
      if (cmd === 'plugin:event|listen') {
        const event = (args || {}).event;
        const handler = (args || {}).handler;
        const list = window.__TAURI_INTERNALS__.__kamuxEventHandlers[event] || [];
        list.push(handler);
        window.__TAURI_INTERNALS__.__kamuxEventHandlers[event] = list;
        return Promise.resolve(list.length);
      }
      if (cmd === 'plugin:event|unlisten') {
        removeListener((args || {}).event, (args || {}).eventId);
        return Promise.resolve(null);
      }
      const h = handlers[cmd];
      if (h === undefined) return Promise.reject(new Error('unmocked command: ' + cmd));
      return Promise.resolve(h(args || {}));
    };
    window.__TAURI_INTERNALS__.transformCallback = (cb) => cb;
    // @tauri-apps/api v2 の unlisten は invoke の前に
    // window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener() を**同期に**呼ぶ
    // （node_modules/@tauri-apps/api/event.js の _unlisten）。これを用意しないと
    // 解除のたびに TypeError になり、呼び出し側から見ると unlisten が失敗する
    // （M3-1 Task 7 の EditorSurface が StrictMode の再マウントで踏んだ）
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (event, eventId) => {
        removeListener(event, eventId);
      },
    };
    window.__TAURI_INTERNALS__.__kamuxEmit = (event, payload) => {
      const list = window.__TAURI_INTERNALS__.__kamuxEventHandlers[event] || [];
      list.forEach((handler) => handler({ event, id: 0, payload }));
    };
  `;
}
