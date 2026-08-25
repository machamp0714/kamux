import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { ArchivedDrawer } from './ArchivedDrawer';
import type { Session } from '../../types/model';

afterEach(cleanup);

const s = (id: string, archivedAt: number | null): Session => ({
  id,
  project_id: 'p1',
  title: `task ${id}`,
  description: '',
  kanban_status: 'done',
  sort_order: 1,
  mode: 'worktree',
  branch: 'session/x',
  worktree_path: '/repo/a/.worktrees/session-x',
  cli_kind: 'claude',
  cli_command: null,
  claude_session_id: null,
  last_runtime_state: 'idle',
  last_runtime_error: null,
  first_started_at: 1,
  heuristics_enabled: true,
  silence_timeout_secs: 30,
  is_scratch: false,
  archived_at: archivedAt,
  created_at: 0,
  updated_at: 0,
});

describe('ArchivedDrawer', () => {
  it('open が false のときは何も描画しない', () => {
    const { container } = render(
      <ArchivedDrawer
        open={false}
        sessions={[s('a', 1)]}
        onRestore={vi.fn()}
        onCleanup={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    expect(container.firstChild).toBeNull();
  });

  it('アーカイブ済みセッションだけを一覧する', () => {
    render(
      <ArchivedDrawer
        open
        sessions={[s('a', 1754006400000), s('b', null)]}
        onRestore={vi.fn()}
        onCleanup={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    expect(screen.getByText('task a')).toBeTruthy();
    expect(screen.queryByText('task b')).toBeNull();
  });

  // レビュー I-2: セッション id をフィクスチャで頻用される 'a' と別の値にし、実装が
  // 渡された id をそのまま転送していることを主張する（定数 'a' 固定でも通る形にしない）。
  it('復元ボタンで onRestore が id 付きで呼ばれる', () => {
    const onRestore = vi.fn();
    render(
      <ArchivedDrawer
        open
        sessions={[s('restore-drawer-9', 1754006400000)]}
        onRestore={onRestore}
        onCleanup={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: '復元' }));
    expect(onRestore).toHaveBeenCalledWith('restore-drawer-9');
  });

  // 裁定 62: KanbanCardCleanup.tsx と同じ aria-label を持たせ、同じ操作の 2 つの
  // ボタンでアクセシブル名が食い違わないようにする。可視ラベルは 🧹 worktree のまま。
  it('worktree が残っているアーカイブ済みには掃除ボタンを出す', () => {
    const onCleanup = vi.fn();
    render(
      <ArchivedDrawer
        open
        sessions={[s('a', 1754006400000)]}
        onRestore={vi.fn()}
        onCleanup={onCleanup}
        onClose={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'worktree を掃除' }));
    expect(onCleanup).toHaveBeenCalledWith('a');
  });

  it('アーカイブが 0 件なら空の案内を出す', () => {
    render(
      <ArchivedDrawer
        open
        sessions={[s('b', null)]}
        onRestore={vi.fn()}
        onCleanup={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    expect(screen.getByText('アーカイブ済みのセッションはありません')).toBeTruthy();
  });

  // 裁定 60: 並び替え（新しい順）が 1 本もテストされていなかった穴を塞ぐ。
  // 2 件だけを渡した既存の 5 件はどれもソート削除の変異で死なない。
  it('アーカイブが新しい順に並ぶ', () => {
    render(
      <ArchivedDrawer
        open
        sessions={[s('old', 1754000000000), s('new', 1754006400000)]}
        onRestore={vi.fn()}
        onCleanup={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    const titles = screen.getAllByText(/^task /).map((el) => el.textContent);
    expect(titles).toEqual(['task new', 'task old']);
  });

  // レビュー Important-3: 閉じる・掃除の配線 4 経路のうち、ここで測るのは
  // component 側の 2 経路（閉じるボタン / スクリム）。
  it('閉じるボタンを押すと onClose が呼ばれる', () => {
    const onClose = vi.fn();
    render(
      <ArchivedDrawer
        open
        sessions={[s('a', 1754006400000)]}
        onRestore={vi.fn()}
        onCleanup={vi.fn()}
        onClose={onClose}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: '閉じる' }));
    expect(onClose).toHaveBeenCalled();
  });

  it('スクリムを押すと onClose が呼ばれる。ドロワー本体を押しても呼ばれない', () => {
    const onClose = vi.fn();
    const { container } = render(
      <ArchivedDrawer
        open
        sessions={[s('a', 1754006400000)]}
        onRestore={vi.fn()}
        onCleanup={vi.fn()}
        onClose={onClose}
      />,
    );

    fireEvent.mouseDown(screen.getByRole('complementary', { name: 'アーカイブ済み' }));
    expect(onClose).not.toHaveBeenCalled();

    const scrim = container.querySelector('.archived-drawer__scrim');
    if (scrim === null) throw new Error('scrim が見つからない');
    fireEvent.mouseDown(scrim);
    expect(onClose).toHaveBeenCalled();
  });
});
