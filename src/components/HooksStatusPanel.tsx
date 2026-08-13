import { useEffect, useState } from 'react';
import { getHooksDiagnostics } from '../ipc/commands';
import type { HookLiveness, HooksDiagnostics } from '../ipc/commands';
import './HooksStatusPanel.css';

export function livenessLabel(liveness: HookLiveness): string {
  switch (liveness) {
    case 'healthy':
      return '疎通OK';
    case 'pending':
      return '確認中';
    case 'unreachable':
      return '不達';
    case 'not_applicable':
      return '対象外';
  }
}

/**
 * 設計書 §12「設定画面に hooks 疎通ステータスを表示」。
 * マウント時に 1 回だけ取得する。定期リフレッシュはしない（契約 §0 のポーリング禁止）。
 * 唯一の呼び出し側（KanbanView/index.tsx）はドロワーの開閉でマウント / アンマウントされるため
 * 開くたびに取得は走るが、開いたままの更新は起きない。
 * `session://state` 受信時の再取得は未実装の将来案である
 * （現在 `session://state/{session_id}` を購読するのは `src/hooks/useRuntimeStateEvents.ts` だけで、
 *  本パネルには結線されていない）。
 */
export function HooksStatusPanel({ sessionTitles }: { sessionTitles: Record<string, string> }) {
  const [diag, setDiag] = useState<HooksDiagnostics | null>(null);

  useEffect(() => {
    let cancelled = false;
    getHooksDiagnostics()
      .then((d) => {
        if (!cancelled) setDiag(d);
      })
      .catch(() => {
        if (!cancelled) setDiag(null);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!diag) return <p className="hooks-status__loading">hooks の状態を取得中…</p>;

  return (
    <section className="hooks-status">
      <h3 className="hooks-status__heading">hooks 疎通ステータス</h3>

      <dl className="hooks-status__summary">
        <dt>ソケット</dt>
        <dd className="hooks-status__path" data-testid="hooks-socket-path">
          {diag.socket_path}
        </dd>
        <dt>リスナ</dt>
        <dd data-testid="hooks-listener">
          {diag.listener_alive ? '稼働中' : '停止（全セッションが推定検知に切り替わります）'}
        </dd>
      </dl>

      {diag.sessions.length === 0 ? (
        <p className="hooks-status__empty" data-testid="hooks-empty">
          実行中のセッションはありません。セッションを開始すると、ここに hooks
          が届いているかどうかが並びます。
        </p>
      ) : (
        <ul className="hooks-status__list">
          {diag.sessions.map((s) => (
            <li
              key={s.session_id}
              className="hooks-status__row"
              data-testid={`hooks-row-${s.session_id}`}
            >
              <span className="hooks-status__title">
                {sessionTitles[s.session_id] ?? s.session_id}
              </span>
              <span className="hooks-status__cli">[{s.cli_kind}]</span>
              <span className="hooks-status__liveness">{livenessLabel(s.liveness)}</span>
              <span className="hooks-status__heuristics">
                {s.heuristics_active ? 'ヒューリスティック稼働中' : '推定は使用していません'}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
