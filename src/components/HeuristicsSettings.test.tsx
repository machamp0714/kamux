import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { HeuristicsSettings, detectionMethodLabel } from './HeuristicsSettings';
import type { Session } from '../types/model';

// vitest の globals は無効（vite.config.ts）。RTL の自動 cleanup はそれに依存するため、
// 明示的に afterEach で片付ける（RuntimeBadge.test.tsx と同じ形）。
afterEach(cleanup);

function session(overrides: Partial<Session> = {}): Session {
  return {
    id: 's1',
    project_id: 'p1',
    title: 'fix login',
    description: '',
    kanban_status: 'in_progress',
    sort_order: 1,
    mode: 'in_place',
    branch: null,
    worktree_path: null,
    cli_kind: 'custom',
    cli_command: null,
    claude_session_id: null,
    last_runtime_state: 'idle',
    last_runtime_error: null,
    first_started_at: 1,
    heuristics_enabled: true,
    silence_timeout_secs: 30,
    archived_at: null,
    created_at: 0,
    updated_at: 0,
    ...overrides,
  };
}

describe('detectionMethodLabel', () => {
  it('reports hooks as the certain method', () => {
    expect(detectionMethodLabel(session({ cli_kind: 'claude' }), 'healthy')).toBe('hooks（確実）');
  });

  it('reports heuristics when hooks are unreachable', () => {
    expect(detectionMethodLabel(session({ cli_kind: 'claude' }), 'unreachable')).toBe(
      'ヒューリスティック（推定）',
    );
  });

  it('reports waiting while inside the grace window', () => {
    expect(detectionMethodLabel(session({ cli_kind: 'claude' }), 'pending')).toBe(
      'hooks の疎通を確認中',
    );
  });

  // brief 原文のテスト名は「reports heuristics for non-claude CLIs」だったが、
  // detectionMethodLabel は session.cli_kind を一度も読まない（§30.4 / §30.6 の
  // 判定順序どおり liveness だけで決める）。cli_kind の差し替えは判別力ゼロなので、
  // liveness を主語にした名前へ直す（task-17-brief 読み替え #7）。
  it('reports heuristics when hooks do not apply', () => {
    expect(detectionMethodLabel(session({ cli_kind: 'custom' }), 'not_applicable')).toBe(
      'ヒューリスティック（推定）',
    );
  });

  it('reports disabled regardless of liveness', () => {
    expect(detectionMethodLabel(session({ heuristics_enabled: false }), 'unreachable')).toBe(
      '無効',
    );
  });
});

describe('HeuristicsSettings', () => {
  it('reflects the current enabled state in the toggle', () => {
    render(<HeuristicsSettings session={session()} onChange={() => {}} />);
    expect(screen.getByLabelText('ヒューリスティック検知')).toBeChecked();
  });

  it('shows the toggle off for a shell session', () => {
    render(
      <HeuristicsSettings
        session={session({ cli_kind: 'shell', heuristics_enabled: false })}
        onChange={() => {}}
      />,
    );
    expect(screen.getByLabelText('ヒューリスティック検知')).not.toBeChecked();
  });

  it('emits a patch when the toggle is flipped', () => {
    const onChange = vi.fn();
    render(<HeuristicsSettings session={session()} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText('ヒューリスティック検知'));
    expect(onChange).toHaveBeenCalledWith({ heuristics_enabled: false });
  });

  it('emits a patch when the timeout is changed', () => {
    const onChange = vi.fn();
    render(<HeuristicsSettings session={session()} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText('沈黙とみなす秒数'), { target: { value: '120' } });
    expect(onChange).toHaveBeenCalledWith({ silence_timeout_secs: 120 });
  });

  it('rejects a timeout below the minimum without emitting', () => {
    const onChange = vi.fn();
    render(<HeuristicsSettings session={session()} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText('沈黙とみなす秒数'), { target: { value: '4' } });
    expect(onChange).not.toHaveBeenCalled();
    // '5' と '3600' を別々に contain するだけだと、テンプレート内で MIN/MAX を
    // 入れ替える変異（"3600〜5 秒"）も両方の contain を満たして生き残る
    // （契約 §81 条件1+2: 同型の 2 値、名前では順序を判別できない）。
    // 順序ごと固定する（advisor 指摘。リテラルで書き、定数からは再導出しない）。
    expect(screen.getByRole('alert').textContent).toContain('5〜3600');
  });

  it('accepts the minimum boundary value', () => {
    const onChange = vi.fn();
    render(<HeuristicsSettings session={session()} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText('沈黙とみなす秒数'), { target: { value: '5' } });
    expect(onChange).toHaveBeenCalledWith({ silence_timeout_secs: 5 });
  });

  it('rejects a timeout above the maximum without emitting', () => {
    const onChange = vi.fn();
    render(<HeuristicsSettings session={session()} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText('沈黙とみなす秒数'), { target: { value: '3601' } });
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toBeTruthy();
  });

  it('accepts the maximum boundary value', () => {
    const onChange = vi.fn();
    render(<HeuristicsSettings session={session()} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText('沈黙とみなす秒数'), { target: { value: '3600' } });
    expect(onChange).toHaveBeenCalledWith({ silence_timeout_secs: 3600 });
  });

  it('disables the timeout input when heuristics are off', () => {
    render(
      <HeuristicsSettings session={session({ heuristics_enabled: false })} onChange={() => {}} />,
    );
    expect(screen.getByLabelText('沈黙とみなす秒数')).toBeDisabled();
  });

  it('states the accuracy limits with the configured timeout', () => {
    render(
      <HeuristicsSettings session={session({ silence_timeout_secs: 45 })} onChange={() => {}} />,
    );
    const note = screen.getByTestId('heuristics-accuracy-note').textContent ?? '';
    expect(note).toContain('推定');
    expect(note).toContain('45 秒');
    expect(note).toContain('ベル文字');
    // 契約 §30.6 / task-17-brief 読み替え #1: 破線ではなく中空ドット + `~` 前置が
    // 確定仕様（RuntimeBadge.css §76.1）。破線という文言は実装に存在しない。
    expect(note).toContain('中空');
    expect(note).toContain('~');
  });
});
