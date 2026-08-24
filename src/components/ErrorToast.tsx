import { useAppStore } from '../store';
import type { AppErrorCode } from '../types/model';
import './ErrorToast.css';

// 契約 §130.6。先例: src/views/KanbanView/badge.ts の RUNTIME_BADGE_LABEL と同じ形
// （短い日本語の名詞句）。7 値は AppErrorCode の union（src/types/model.ts）と 1:1。
export const APP_ERROR_LABEL: Record<AppErrorCode, string> = {
  db: 'データベースエラー',
  not_found: '対象が見つかりません',
  pty_spawn: '端末の起動に失敗',
  git: 'Git エラー',
  cli_not_found: 'CLI が見つかりません',
  invalid_state: '不正な状態',
  io: '入出力エラー',
};

/**
 * 設計書 §12「stderr をトーストでそのまま表示」。message は加工しない（契約 §6）。
 */
export function ErrorToast() {
  const lastError = useAppStore((s) => s.lastError);
  const setError = useAppStore((s) => s.setError);
  if (lastError === null) return null;

  // 対応表に無い値なら要素ごと出さない（空文字の要素も残さない）。生の code が
  // 隣に在るので情報は落ちない。
  const label = APP_ERROR_LABEL[lastError.code] as string | undefined;

  return (
    <div className="error-toast" role="alert">
      <span className="error-toast__code">{lastError.code}</span>
      {label !== undefined && <span className="error-toast__label">{label}</span>}
      <pre className="error-toast__message">{lastError.message}</pre>
      <button
        type="button"
        className="error-toast__close"
        onClick={() => setError(null)}
        aria-label="閉じる"
      >
        ×
      </button>
    </div>
  );
}
