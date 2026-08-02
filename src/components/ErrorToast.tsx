import { useAppStore } from '../store';
import './ErrorToast.css';

/**
 * 設計書 §12「stderr をトーストでそのまま表示」。message は加工しない（契約 §6）。
 */
export function ErrorToast() {
  const lastError = useAppStore((s) => s.lastError);
  const setError = useAppStore((s) => s.setError);
  if (lastError === null) return null;

  return (
    <div className="error-toast" role="alert">
      <span className="error-toast__code">{lastError.code}</span>
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
