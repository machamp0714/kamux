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
    };
  }
}

/**
 * window.__TAURI_INTERNALS__.invoke を差し替えるスクリプトを組み立てる。
 * page.addInitScript() はブラウザ側で関数を再評価するだけで、渡した関数のクロージャは
 * 持ち込めない。そのため tauriMockScript 自身は文字列を返す純関数にし、
 * spec.commands の各関数本体は toString() で文字列化して埋め込む
 * ——**渡す関数は外側の変数を参照しないこと**（参照するとブラウザ側で undefined になる）。
 */
export function tauriMockScript(spec: TauriMockSpec): string {
  const entries = Object.entries(spec.commands)
    .map(([name, fn]) => `${JSON.stringify(name)}: (${fn.toString()})`)
    .join(',\n');
  return `
    window.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__ || {};
    window.__TAURI_INTERNALS__.__kamuxCalls = [];
    const handlers = { ${entries} };
    window.__TAURI_INTERNALS__.invoke = (cmd, args) => {
      window.__TAURI_INTERNALS__.__kamuxCalls.push({ cmd, args });
      const h = handlers[cmd];
      if (h === undefined) return Promise.reject(new Error('unmocked command: ' + cmd));
      return Promise.resolve(h(args || {}));
    };
    window.__TAURI_INTERNALS__.transformCallback = (cb) => cb;
  `;
}
