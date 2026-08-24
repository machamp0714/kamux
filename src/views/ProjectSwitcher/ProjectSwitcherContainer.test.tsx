import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const listSessions = vi.fn();
vi.mock('../../ipc/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../ipc/commands')>()),
  listSessions: (...a: unknown[]) => listSessions(...a),
}));

import { useAppStore } from '../../store';
import type { Project } from '../../types/model';
import { ProjectSwitcherContainer } from './ProjectSwitcherContainer';

afterEach(cleanup);

const project = (id: string, name: string): Project => ({
  id,
  name,
  repo_path: `/repo/${name}`,
  default_cli: 'claude',
  created_at: 0,
  updated_at: 0,
});

beforeEach(() => {
  listSessions.mockReset().mockResolvedValue([]);
  useAppStore.setState({
    projects: [project('p1', 'kamux'), project('p2', 'beta')],
    activeProjectId: 'p1',
    projectSwitcherOpen: false,
  });
});

describe('ProjectSwitcherContainer', () => {
  it('projectSwitcherOpen が false なら何も描かない', () => {
    const { container } = render(<ProjectSwitcherContainer />);
    expect(container.firstChild).toBeNull();
  });

  it('開いているときは候補を描く', () => {
    useAppStore.setState({ projectSwitcherOpen: true });
    render(<ProjectSwitcherContainer />);
    expect(screen.getAllByRole('option')).toHaveLength(2);
  });

  it('候補を選ぶとスイッチャーを閉じ、そのプロジェクトをアクティブにする', async () => {
    useAppStore.setState({ projectSwitcherOpen: true });
    render(<ProjectSwitcherContainer />);

    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'beta' } });
    fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' });

    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
    await waitFor(() => expect(useAppStore.getState().activeProjectId).toBe('p2'));
    // 取り違えたら別物になる具体値で観測する（activeProjectId と同じ素の string が
    // 2 本ある経路。'p1' を渡す実装でも呼び出し自体は起きる）。
    expect(listSessions).toHaveBeenCalledWith('p2', true);
  });

  it('Escape で閉じる（アクティブプロジェクトは動かない）', () => {
    useAppStore.setState({ projectSwitcherOpen: true });
    render(<ProjectSwitcherContainer />);

    fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Escape' });

    expect(useAppStore.getState().projectSwitcherOpen).toBe(false);
    expect(useAppStore.getState().activeProjectId).toBe('p1');
  });
});
