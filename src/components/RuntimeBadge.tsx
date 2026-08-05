import { useAppStore } from '../store';
import type { RuntimeState, StateReason } from '../types/model';
import './RuntimeBadge.css';

/**
 * 契約 §33.5 のラベル正典。**6 値すべてを埋めること**
 * （Record<RuntimeState, string> なので 5 値では型エラーになる）。
 * 色は CSS 側の `--state-*`（契約 §53.4）が正典で、ここには持たない。
 */
const RUNTIME_LABEL: Record<RuntimeState, string> = {
  running: '実行中',
  waiting_input: '入力待ち',
  idle: 'アイドル',
  exited: '終了',
  interrupted: '中断',
  error: 'エラー',
};

/**
 * 契約 §53.4 の色の正典。**`--state-{state}` を機械的に組み立てないこと** ——
 * `waiting_input` のトークンだけ `_input` が付かず、組み立てるとここだけ
 * 存在しないトークンを指して色が落ちる。
 */
const RUNTIME_STATE_TOKEN: Record<RuntimeState, string> = {
  running: '--state-running',
  waiting_input: '--state-waiting',
  idle: '--state-idle',
  exited: '--state-exited',
  interrupted: '--state-interrupted',
  error: '--state-error',
};

/**
 * 純粋な描画（契約 §25.5）。store に触らないのでテストから直接 render できる。
 *
 * デザインシステム「実行状態バッジ」節: 色だけで状態を示さず、**必ずドット + ラベル**。
 * 背景も枠も持たない（ピル型にしない）。
 */
export function RuntimeBadgeView({
  state,
  reason,
}: {
  state: RuntimeState;
  reason?: StateReason;
}): JSX.Element {
  const label = RUNTIME_LABEL[state];
  return (
    <span
      className="runtime-badge"
      data-runtime-state={state}
      role="img"
      aria-label={label}
      title={reason === undefined ? label : `${label} (${reason})`}
      // 色はトークンへの参照だけを載せる（値は tokens.css がテーマごとに持つ）。
      // ドットとラベルは CSS 側で currentColor として受け取る
      style={{ color: `var(${RUNTIME_STATE_TOKEN[state]})` }}
    >
      <span className="runtime-badge__dot" />
      <span className="runtime-badge__label">{label}</span>
    </span>
  );
}

/**
 * runtime_state の唯一の視覚表現（設計書 §6.2）。
 *
 * **`runtimeStates` / `runtimeReasons` を購読してよいのは契約 §38.3 の許可リストに
 * 載ったファイルだけで、バッジを描く購読者はこれ 1 つである。**
 * selector はプリミティブ文字列を返すので、無関係なセッションの遷移では
 * 再レンダリングされない。`useAppStore((s) => s.runtimeStates)` のような
 * オブジェクト全体の購読は禁止。
 *
 * kanban_status（カードがどの列にいるか）とは完全に独立している。
 * このコンポーネントは列の位置に一切影響しない。
 */
export function RuntimeBadge({ sessionId }: { sessionId: string }): JSX.Element | null {
  const state = useAppStore((s) => s.runtimeStates[sessionId]);
  const reason = useAppStore((s) => s.runtimeReasons[sessionId]);

  // 契約 §33.3 Q1 / §34.7: 状態が未知（= 一度も起動していない、または最初の
  // session://state 到着前）のときは描画しない。`?? 'idle'` は禁止 —— ⚪ は §2 で
  // 「Stop hook 受信」を意味する確定値であり、未知であることの表現ではない。
  if (state === undefined) return null;

  return <RuntimeBadgeView state={state} reason={reason} />;
}
