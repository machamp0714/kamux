// 契約 §28.1: Layout の正典は src/types/model.ts。ここでは import して再エクスポートする
// （types/model.ts が TerminalWorkspace で Layout を使うため、ここに定義すると
//  types → store の逆向き依存になる）
import type { Layout } from '../types/model';
export type { Layout };

/**
 * 2 ペインが同時に見えているか。向き（左右 / 上下）は問わない（契約 §28.2）。
 * レイアウト比較は例外なくこの関数を通すこと。個別に
 * `=== 'split2' || === 'split2-v'` と書くと、書き漏らした箇所だけが
 * 縦分割で single 扱いになり、症状が箇所ごとに違って切り分け不能になる。
 */
export function isSplit(layout: Layout): boolean {
  return layout !== 'single';
}

/** Cmd+D の 3 値サイクル（契約 §28.5）。キーマップ側に三項演算子を書かない。 */
export function nextLayout(l: Layout): Layout {
  return l === 'single' ? 'split2' : l === 'split2' ? 'split2-v' : 'single';
}

/** 永続化された値の検証（契約 §28.6）。M3-4 の workspace 復元が使う。 */
export function isLayout(v: unknown): v is Layout {
  return v === 'single' || v === 'split2' || v === 'split2-v';
}
export type PaneIndex = 0 | 1;
export type PaneAssignment = [string | null, string | null];

/**
 * ターミナル画面のペイン状態。契約 §10 terminalSlice の 3 フィールドと 1:1。
 * 不変条件:
 *   1. paneAssignment[0] !== paneAssignment[1]（両方が非 null のとき）
 *   2. single のとき表示されるのは paneAssignment[activePane] のみ
 */
export interface PaneState {
  layout: Layout;
  paneAssignment: PaneAssignment;
  activePane: PaneIndex;
}

export const otherPane = (pane: PaneIndex): PaneIndex => (pane === 0 ? 1 : 0);

/** 画面に描画されるペインを左から並べて返す。 */
export function visiblePanes(s: PaneState): PaneIndex[] {
  return isSplit(s.layout) ? [0, 1] : [s.activePane];
}

/**
 * ペインへセッションを割り当て、そのペインをアクティブにする。
 * 要求されたセッションがもう一方のペインに居る場合は 2 ペインの割当を交換し、
 * 同一セッションが 2 ペインに存在しないという不変条件を守る。
 *
 * single では addressable なペインが activePane しか無いため、pane 引数を
 * 無視して activePane へ寄せる（setActivePaneReducer の no-op と対称）。
 * これにより「single の間 activePane は決して動かない」という不変条件 4 が
 * どの reducer 経由でも成立し、TerminalGrid のホスト要素が再マウントされない。
 */
export function assignPaneReducer(s: PaneState, pane: PaneIndex, sessionId: string): PaneState {
  const target: PaneIndex = isSplit(s.layout) ? pane : s.activePane;

  if (s.paneAssignment[target] === sessionId) {
    return { layout: s.layout, paneAssignment: s.paneAssignment, activePane: target };
  }

  const other = otherPane(target);
  const paneAssignment: PaneAssignment = [s.paneAssignment[0], s.paneAssignment[1]];

  if (s.paneAssignment[other] === sessionId) {
    paneAssignment[other] = s.paneAssignment[target];
  }
  paneAssignment[target] = sessionId;

  return { layout: s.layout, paneAssignment, activePane: target };
}

/**
 * レイアウトのみを切り替える。paneAssignment / activePane は保持する。
 * single に落としても裏スロットの割当を捨てないので、split2 に戻すと
 * 左右の位置ごと元通りになる（設計 §3.5）。
 */
export function setLayoutReducer(s: PaneState, layout: Layout): PaneState {
  if (s.layout === layout) return s;
  return { layout, paneAssignment: s.paneAssignment, activePane: s.activePane };
}

/**
 * アクティブペインを移す。single では「ペイン」がユーザーの画面に存在しないため
 * no-op（設計 §3.5 付随決定）。
 */
export function setActivePaneReducer(s: PaneState, pane: PaneIndex): PaneState {
  if (!isSplit(s.layout)) return s;
  if (s.activePane === pane) return s;
  return { layout: s.layout, paneAssignment: s.paneAssignment, activePane: pane };
}

/**
 * タブ順を dir 方向に 1 つ進めた session_id を返す。
 * exclude に含まれる id は候補から外す。候補が無ければ null。
 * order はレイアウトに一切依存しない（呼び出し側が selectTerminalTabs 等で渡す）。
 */
export function nextSessionId(
  order: string[],
  current: string | null,
  dir: 1 | -1,
  exclude: string[],
): string | null {
  const candidates = order.filter((id) => !exclude.includes(id));
  if (candidates.length === 0) return null;

  const i = current === null ? -1 : candidates.indexOf(current);
  if (i === -1) return dir === 1 ? candidates[0] : candidates[candidates.length - 1];

  return candidates[(i + dir + candidates.length) % candidates.length];
}

/**
 * フォーカス中ペインのセッションをタブ順で 1 つ動かす（Cmd+J / Cmd+K）。
 *
 * 除外条件は「もう一方のスロットが非 null か」ではなく isSplit(layout) か（契約 §28.2）。
 * single では裏スロットの割当が画面に出ていないため、これを除外すると
 * そのセッションが Cmd+J/K で到達不能になる（設計 §3.6）。
 */
export function cycleSessionReducer(s: PaneState, order: string[], dir: 1 | -1): PaneState {
  const hidden = isSplit(s.layout) ? s.paneAssignment[otherPane(s.activePane)] : null;
  const exclude = hidden === null ? [] : [hidden];

  const next = nextSessionId(order, s.paneAssignment[s.activePane], dir, exclude);
  if (next === null || next === s.paneAssignment[s.activePane]) return s;

  // assignPaneReducer を経由することで、single のときに裏スロットのセッションへ
  // 巡回してもスワップになり、同一セッションが両スロットに入らない。
  return assignPaneReducer(s, s.activePane, next);
}

/**
 * 「このセッションを見せろ」という要求をペイン操作へ翻訳する
 * （カードクリック / focus://session/{session_id} / エディタ画面のタブ切替）。
 *
 * split2 / split2-v でもう一方のペインに既に出ている場合は割当を動かさず
 * activePane だけ移す。これが設計書 要件 5「該当セッションのペインに
 * フォーカスが当たる」。レイアウト比較は例外なく isSplit() を通す（契約 §28.2）。
 */
export function routeFocusReducer(s: PaneState, sessionId: string): PaneState {
  if (s.paneAssignment[s.activePane] === sessionId) return s;

  if (isSplit(s.layout) && s.paneAssignment[otherPane(s.activePane)] === sessionId) {
    return {
      layout: s.layout,
      paneAssignment: s.paneAssignment,
      activePane: otherPane(s.activePane),
    };
  }

  return assignPaneReducer(s, s.activePane, sessionId);
}
