import type { Terminal } from '@xterm/xterm';

export type KeyRoute = 'terminal' | 'app';

export interface KeyRoutingInput {
  type: string;
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/**
 * キーイベントを xterm（= nvim / CLI）とアプリのどちらが処理するかを決める。
 *
 * nvim は Cmd を一切使わないので、境界は metaKey 一本で引ける。
 * KeyRoutingInput の type / ctrlKey / altKey / shiftKey は判定に使っていない。
 * 型に残してあるのは、D1 の境界（Esc / Ctrl-W / Tab / Alt-x はターミナル、
 * Cmd+C / Cmd+V はアプリ）をテストの表として書けるようにするためである（契約 §77.2）。
 *
 * 'app' を返した場合 xterm はキーを処理せず preventDefault も呼ばないため、
 * (a) ブラウザ既定の copy / paste が走り (b) window の keydown リスナに到達する。
 * 'app' に振ったキーを実際に消費するかは resolveKeymap（src/hooks/keymap.ts）が決める。
 */
export function routeKeyEvent(input: KeyRoutingInput): KeyRoute {
  return input.metaKey ? 'app' : 'terminal';
}

/**
 * 契約 §65.6 の「修飾キーのみ」の最小集合 4 つに `CapsLock` を加えた 5 つ。
 *
 * `CapsLock` を含める判断は §65.6 がレーンの裁量とした点（lane-controller の裁定）:
 * spike（実機 WKWebView）でこの環境には文字入力の `keypress` 経路が存在しないと
 * 分かったため、`_keyDownSeen` を落とす副作用は `_inputEvent` のガードを通す
 * だけになり、二重入力の経路が無い。一方 macOS の `CapsLock` は対になる `keyup`
 * が同じ打鍵で来るとは限らず、来なければ `_keyDownSeen` が `true` のまま居座る。
 * 落とす側に危険が無く、落とさない側に居座りの危険があるので含める。
 * `AltGraph` は macOS の対象環境に存在しないため含めない。
 */
const MODIFIER_ONLY_KEYS = new Set(['Shift', 'Control', 'Alt', 'Meta', 'CapsLock']);

/** `term._core` の必要な形だけを局所的に型付けする（契約 §65.12: 実体は `_core` の下） */
interface XtermCoreKeyState {
  _keyDownSeen: boolean;
}

/**
 * `core != null` だけでは緩すぎる（fix round 1 Minor 3）: 版が上がって
 * `_keyDownSeen` の型だけ変わった場合（例: 文字列やオブジェクトに変わる）を
 * 弾くために `typeof ... === 'boolean'` まで見る。緩めると §65.6 T8 の亜種
 * （`_core` はあるが `_keyDownSeen` が boolean でない偽 term）が検出できなくなる。
 */
function hasKeyDownSeenFlag(core: unknown): core is XtermCoreKeyState {
  return (
    typeof core === 'object' &&
    core !== null &&
    typeof (core as { _keyDownSeen?: unknown })._keyDownSeen === 'boolean'
  );
}

/**
 * 契約 §65.3: `@xterm/xterm` 5.5.0 の上流バグ（WKWebView + IME で `_keyDownSeen` が
 * keydown のたびに立ったままになり、`_inputEvent` のガードが Shift 修飾の 1 文字目
 * ── `!` `@` に加え、実機 spike（2026-08-05）で判明した Shift+英字も含む ── を
 * 飲んでしまう）への手当て。修飾キーのみの `keydown` で `_keyDownSeen` を `false` へ
 * 戻す（上流 PR #6054 の `wasModifierKeyOnlyEvent` と同じ意味論）。
 *
 * フラグの書き戻しは副作用として行う。返り値（Cmd 系の奪取）だけでは直らない
 * —— `_keyDownSeen=true` は custom handler の呼び出しより前に xterm 自身が走らせる
 * ため（契約 §65.2）。
 *
 * I2: 修飾キー以外の keydown には触らない。I4: keyup / keypress から呼ばれた
 * ときは触らない（xterm 自身が `_keyUp` の先頭で false にしている。二重に書くと
 * どちらが書いたか読めなくなる）。
 */
function resetKeyDownSeenIfModifierOnly(term: Terminal, event: KeyboardEvent): void {
  if (event.type !== 'keydown') return; // I4
  if (!MODIFIER_ONLY_KEYS.has(event.key)) return; // I2
  // `_core` が無い / フラグが boolean でない場合は何もしない（版が上がって形が
  // 変わったときに例外で落とさない。検出は xtermCanary.test.ts の役目）
  const core = (term as unknown as { _core?: unknown })._core;
  if (hasKeyDownSeenFlag(core)) {
    core._keyDownSeen = false; // I1
  }
}

/**
 * xterm.js の attachCustomKeyEventHandler に渡すハンドラを作る。
 *
 * ⚠️ 契約 §65.9 / §77: これは返り値だけの述語ではない。修飾キーのみの keydown で
 * xterm 内部の `_keyDownSeen` を false へ戻す副作用を持つ（契約 §65.3 / PR H1）。
 * 副作用は term インスタンスを必須とするので、素の (e) => boolean では表現できない。
 * 返り値だけを写すと WKWebView + IME で `!` / `@` / Shift+英字の 1 打目が再び消える。
 * 不変条件は契約 §65.6 の I1〜I4。
 *
 * 呼ぶのは registry.ts の ensureTerminal ただ 1 箇所である（契約 §16 / §65.6 I3）。
 * 消費側は attachCustomKeyEventHandler を呼ばない —— セッターなので、再設定すると
 * 上書きで副作用が落ちる。
 */
export function createTerminalKeyEventHandler(term: Terminal): (e: KeyboardEvent) => boolean {
  return (event: KeyboardEvent): boolean => {
    resetKeyDownSeenIfModifierOnly(term, event);
    return routeKeyEvent(event) === 'terminal';
  };
}
