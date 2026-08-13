import { useState } from 'react';
import type { HookLiveness } from '../ipc/commands';
import type { Session } from '../types/model';

export const MIN_SILENCE_TIMEOUT_SECS = 5;
export const MAX_SILENCE_TIMEOUT_SECS = 3600;

export interface HeuristicsPatch {
  heuristics_enabled?: boolean;
  silence_timeout_secs?: number;
}

/** 現在このセッションの状態が何によって決まっているか（設計 §4.9 / 契約 §30.4・§30.6）。
 *  session.cli_kind は読まない —— liveness だけで判定順序が決まる（§30.4 参照）。 */
export function detectionMethodLabel(session: Session, liveness: HookLiveness | undefined): string {
  if (!session.heuristics_enabled) return '無効';
  if (liveness === 'healthy') return 'hooks（確実）';
  if (liveness === 'pending') return 'hooks の疎通を確認中';
  return 'ヒューリスティック（推定）';
}

export function HeuristicsSettings({
  session,
  liveness,
  onChange,
}: {
  session: Session;
  liveness?: HookLiveness;
  onChange: (patch: HeuristicsPatch) => void;
}) {
  const [error, setError] = useState<string | null>(null);

  const handleTimeout = (raw: string) => {
    const secs = Number(raw);
    if (
      !Number.isInteger(secs) ||
      secs < MIN_SILENCE_TIMEOUT_SECS ||
      secs > MAX_SILENCE_TIMEOUT_SECS
    ) {
      setError(
        `沈黙とみなす秒数は ${MIN_SILENCE_TIMEOUT_SECS}〜${MAX_SILENCE_TIMEOUT_SECS} 秒で指定してください`,
      );
      return;
    }
    setError(null);
    onChange({ silence_timeout_secs: secs });
  };

  return (
    <fieldset className="heuristics-settings">
      <legend>状態検知</legend>

      <p className="heuristics-settings__method">
        検知方式: <strong>{detectionMethodLabel(session, liveness)}</strong>
      </p>

      <label htmlFor="heuristics-enabled">ヒューリスティック検知</label>
      <input
        id="heuristics-enabled"
        type="checkbox"
        checked={session.heuristics_enabled}
        onChange={(e) => onChange({ heuristics_enabled: e.target.checked })}
      />

      <label htmlFor="silence-timeout">沈黙とみなす秒数</label>
      <input
        id="silence-timeout"
        type="number"
        min={MIN_SILENCE_TIMEOUT_SECS}
        max={MAX_SILENCE_TIMEOUT_SECS}
        defaultValue={session.silence_timeout_secs}
        disabled={!session.heuristics_enabled}
        onChange={(e) => handleTimeout(e.target.value)}
      />

      {error && <p role="alert">{error}</p>}

      {/* 設計書 §9.2「精度限界は UI に明示」。文言は設計 §7 を土台に、契約 §30.6 の
          「hooks が届いているセッションでは」と、推定表示の確定仕様（中空ドット + `~`
          前置。components.md「実行状態バッジ」節 / RuntimeBadge.css §76.1）に揃えてある。
          「破線」という文言は実装に存在しないため使わない（task-17-brief 読み替え #1）。 */}
      <p className="heuristics-settings__note" data-testid="heuristics-accuracy-note">
        Claude Code 以外の CLI では状態を確実に知る手段がないため、次の 2 つの手がかりから
        <strong>推定</strong>します。
        <br />
        ベル文字の検知 — CLI
        が鳴らす通知音を「入力待ち」とみなします。補完の失敗音やエラー音でも反応することがあります。
        <br />
        出力の停止 — {session.silence_timeout_secs} 秒間出力がなければ「アイドル」とみなします。
        長い思考中や、出力を伴わない処理中でもアイドル扱いになります。
        <br />
        推定で付いた状態はバッジのドットが<strong>中空</strong>になり、ラベルの先頭に
        <strong>~</strong>
        が付きます。hooks が届いているセッションでは hooks による確実な検知が優先され、hooks
        が届いている間この推定は使われません。
      </p>
    </fieldset>
  );
}
