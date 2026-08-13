import { useAppStore } from '../store';
import type { RuntimeState, StateReason } from '../types/model';
import { RUNTIME_BADGE_LABEL } from '../views/KanbanView/badge';
import './RuntimeBadge.css';

/**
 * 契約 §33.5 のラベル正典。**6 値すべてを埋めること**
 * （Record<RuntimeState, string> なので 5 値では型エラーになる）。
 * 色は CSS 側の `--state-*`（契約 §53.4）が正典で、ここには持たない。
 *
 * 契約 §76.4: `badge.ts` と本ファイルに同じ表が並んでいたのを統合した。
 * ラベル表の実体は `badge.ts` の `RUNTIME_BADGE_LABEL` に一本化し、
 * ここではローカル再定義しない（片方だけ直す事故を防ぐ）。
 */
const RUNTIME_LABEL = RUNTIME_BADGE_LABEL;

/**
 * ヒューリスティック（汎用 CLI 向け BEL 検知 / 沈黙判定）由来の reason だけを
 * 「推定」として扱う（契約 §33.5 末尾・設計 §4.9）。`StateReason` は 13 値
 * あるため `Record<StateReason, boolean>` で網羅する —— 配列リテラルでは
 * 新しい値が増えても更新が強制されない（M3-3 Task 15 の先例。
 * sessionSlice.heuristics.test.ts の ALL_REASONS と同じ形）。
 */
const ESTIMATED_REASON: Record<StateReason, boolean> = {
  spawned: false,
  hook_notification: false,
  hook_stop: false,
  pty_exited: false,
  startup_normalize: false,
  bel_detected: true,
  silence_timeout: true,
  user_stopped: false,
  output_activity: false,
  user_input: false,
  hook_permission: false,
  resume_failed: false,
  spawn_failed: false,
};

export function isEstimated(reason: StateReason | undefined): boolean {
  return reason !== undefined && ESTIMATED_REASON[reason];
}

/**
 * ツールチップの組み立て正典（契約 §33.5 末尾）。権威ある reason では
 * ラベルのみを返し、ヒューリスティック由来の reason では
 * 「（推定）— 理由。誤検知の注意」を付す。
 */
export function badgeTooltip(state: RuntimeState, reason: StateReason | undefined): string {
  const caveat = '汎用 CLI 向けのヒューリスティックのため誤検知することがあります';
  switch (reason) {
    case 'bel_detected':
      return `${RUNTIME_LABEL[state]}（推定）— ベル文字を検知。${caveat}`;
    case 'silence_timeout':
      return `${RUNTIME_LABEL[state]}（推定）— 出力が一定時間停止。${caveat}`;
    default:
      return RUNTIME_LABEL[state];
  }
}

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
 *
 * M3-3: ヒューリスティック由来の推定状態には `.runtime-badge--estimated`
 * （中空ドット。色は変えない）とラベルへの `~` 前置を足す。この 2 点の確定仕様は
 * `.claude/skills/kamux-design-system/components.md`「実行状態バッジ」節（契約
 * §53.5 が定める 3 層の正典のうち「寸法と役割」層。§76.1 / §76.2。グリフは
 * 描かない）。ツールチップに推定である理由を添える組み立ては badgeTooltip の
 * 正典（契約 §33.5 末尾）に従う。
 */
export function RuntimeBadgeView({
  state,
  reason,
}: {
  state: RuntimeState;
  reason?: StateReason;
}): JSX.Element {
  const label = RUNTIME_LABEL[state];
  const estimated = isEstimated(reason);
  const tooltip = badgeTooltip(state, reason);
  // components.md「実行状態バッジ」節: 推定状態はラベルに `~` を前置する
  // （ドットの中空化だけでは 8×8 の円では視認性が弱いため、テキストにも
  // 推定の合図を持たせる）
  const displayLabel = estimated ? `~${label}` : label;
  return (
    <span
      className={estimated ? 'runtime-badge runtime-badge--estimated' : 'runtime-badge'}
      data-runtime-state={state}
      data-estimated={estimated ? 'true' : 'false'}
      role="img"
      // 推定であることを読み上げにも乗せる。権威ある reason では tooltip === label
      // なので既存の toHaveAccessibleName(label) の期待とは衝突しない
      aria-label={tooltip}
      title={tooltip}
      // 色はトークンへの参照だけを載せる（値は tokens.css がテーマごとに持つ）。
      // ドットとラベルは CSS 側で currentColor として受け取る
      style={{ color: `var(${RUNTIME_STATE_TOKEN[state]})` }}
    >
      <span className="runtime-badge__dot" />
      <span className="runtime-badge__label">{displayLabel}</span>
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
