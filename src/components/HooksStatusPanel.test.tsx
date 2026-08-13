import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// HooksStatusPanel が読む IPC コマンドをモックする（SessionFormModal.test.tsx と同じ形）。
vi.mock('../ipc/commands', () => ({ getHooksDiagnostics: vi.fn() }));

import { getHooksDiagnostics, type HooksDiagnostics } from '../ipc/commands';
import { HooksStatusPanel, livenessLabel } from './HooksStatusPanel';

// このリポジトリは vitest の globals を有効にしていないため RTL の自動 cleanup が
// 登録されない。render を複数回行うので明示的に片付ける。
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const diagnostics = (over: Partial<HooksDiagnostics> = {}): HooksDiagnostics => ({
  socket_path: '/tmp/kamux-hooks-1234.sock',
  listener_alive: true,
  sessions: [],
  ...over,
});

describe('livenessLabel', () => {
  it('maps every liveness value to Japanese', () => {
    expect(livenessLabel('healthy')).toBe('疎通OK');
    expect(livenessLabel('pending')).toBe('確認中');
    expect(livenessLabel('unreachable')).toBe('不達');
    expect(livenessLabel('not_applicable')).toBe('対象外');
  });
});

describe('HooksStatusPanel', () => {
  beforeEach(() => {
    vi.mocked(getHooksDiagnostics).mockReset();
  });

  it('shows the socket path', async () => {
    vi.mocked(getHooksDiagnostics).mockResolvedValue(diagnostics());
    render(<HooksStatusPanel sessionTitles={{}} />);
    await waitFor(() =>
      expect(screen.getByTestId('hooks-socket-path').textContent).toContain(
        '/tmp/kamux-hooks-1234.sock',
      ),
    );
  });

  it('reports a live listener', async () => {
    vi.mocked(getHooksDiagnostics).mockResolvedValue(diagnostics({ listener_alive: true }));
    render(<HooksStatusPanel sessionTitles={{}} />);
    await waitFor(() =>
      expect(screen.getByTestId('hooks-listener').textContent).toContain('稼働中'),
    );
  });

  it('reports a dead listener', async () => {
    vi.mocked(getHooksDiagnostics).mockResolvedValue(diagnostics({ listener_alive: false }));
    render(<HooksStatusPanel sessionTitles={{}} />);
    await waitFor(() => expect(screen.getByTestId('hooks-listener').textContent).toContain('停止'));
  });

  it('lists sessions with their title and liveness', async () => {
    vi.mocked(getHooksDiagnostics).mockResolvedValue(
      diagnostics({
        sessions: [
          {
            session_id: 's1',
            cli_kind: 'claude',
            liveness: 'unreachable',
            last_hook_at: null,
            heuristics_active: true,
          },
        ],
      }),
    );
    render(<HooksStatusPanel sessionTitles={{ s1: 'fix login' }} />);
    await waitFor(() => {
      const row = screen.getByTestId('hooks-row-s1');
      expect(row.textContent).toContain('fix login');
      expect(row.textContent).toContain('不達');
      expect(row.textContent).toContain('ヒューリスティック');
    });
  });

  it('falls back to the session id when the title is unknown', async () => {
    vi.mocked(getHooksDiagnostics).mockResolvedValue(
      diagnostics({
        sessions: [
          {
            session_id: 's9',
            cli_kind: 'codex',
            liveness: 'not_applicable',
            last_hook_at: null,
            heuristics_active: true,
          },
        ],
      }),
    );
    render(<HooksStatusPanel sessionTitles={{}} />);
    await waitFor(() => expect(screen.getByTestId('hooks-row-s9').textContent).toContain('s9'));
  });

  it('shows an empty note when no sessions are running', async () => {
    vi.mocked(getHooksDiagnostics).mockResolvedValue(diagnostics());
    render(<HooksStatusPanel sessionTitles={{}} />);
    await waitFor(() =>
      expect(screen.getByTestId('hooks-empty').textContent).toContain(
        '実行中のセッションはありません',
      ),
    );
  });

  it('fetches exactly once on mount and never polls', async () => {
    vi.mocked(getHooksDiagnostics).mockResolvedValue(diagnostics());
    render(<HooksStatusPanel sessionTitles={{}} />);
    // 取得結果が描画される（= 状態更新による再レンダリングが済む）まで待ってから数える。
    // ここを待たずに数えると useEffect の依存配列 [] を外す変異が緑を通り抜ける
    // ——実測で確認済み。フェイクタイマだけでは再レンダリングまで進まない。
    await screen.findByTestId('hooks-socket-path');
    expect(getHooksDiagnostics).toHaveBeenCalledTimes(1);

    // そのうえで時間を進め、定期リフレッシュが仕掛けられていないことを見る。
    vi.useFakeTimers();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(getHooksDiagnostics).toHaveBeenCalledTimes(1);
  });
});
