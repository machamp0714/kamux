import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { ProjectSwitcher } from './ProjectSwitcher';
import type { Project } from '../../types/model';

afterEach(cleanup);

const p = (id: string, name: string, repoPath: string): Project => ({
  id,
  name,
  repo_path: repoPath,
  default_cli: 'claude',
  created_at: 0,
  updated_at: 0,
});

const projects = [
  p('1', 'kamux', '/Users/me/repo/kamux'),
  p('2', 'beta', '/Users/me/work/beta'),
  p('3', 'alpha', '/Users/me/repo/alpha'),
];

describe('ProjectSwitcher', () => {
  it('初期表示で全プロジェクトを名前順に並べる', () => {
    render(
      <ProjectSwitcher
        projects={projects}
        activeProjectId="1"
        onSelect={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    const items = screen.getAllByRole('option').map((el) => el.textContent);
    expect(items[0]).toContain('alpha');
    expect(items[2]).toContain('kamux');
  });

  it('入力ごとにインクリメンタルに絞り込む', () => {
    render(
      <ProjectSwitcher
        projects={projects}
        activeProjectId="1"
        onSelect={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'kmx' } });
    expect(screen.getAllByRole('option')).toHaveLength(1);
    expect(screen.getByRole('option').textContent).toContain('kamux');
  });

  // filterProjects は大文字小文字を無視する（filterProjects.ts の h / n の toLowerCase()）。
  // filterProjects.test.ts の 15 件の呼び出しは needle / query 側がすべて小文字なので、
  // needle 側の toLowerCase() を落とす変異はあちらでは緑になる（変異検証で確認した）。
  // 大文字クエリの経路をここで押さえる。
  it('大文字のクエリでも小文字の候補にマッチする', () => {
    render(
      <ProjectSwitcher
        projects={projects}
        activeProjectId="1"
        onSelect={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'KMX' } });
    expect(screen.getAllByRole('option')).toHaveLength(1);
    expect(screen.getByRole('option').textContent).toContain('kamux');
  });

  it('Enter で先頭候補を選ぶ', () => {
    const onSelect = vi.fn();
    render(
      <ProjectSwitcher
        projects={projects}
        activeProjectId="1"
        onSelect={onSelect}
        onClose={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'beta' } });
    fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' });
    expect(onSelect).toHaveBeenCalledWith('2');
  });

  it('ArrowDown で選択が下がり、Enter でその候補を選ぶ', () => {
    const onSelect = vi.fn();
    render(
      <ProjectSwitcher
        projects={projects}
        activeProjectId="1"
        onSelect={onSelect}
        onClose={vi.fn()}
      />,
    );
    const input = screen.getByRole('combobox');
    fireEvent.keyDown(input, { key: 'ArrowDown' });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onSelect).toHaveBeenCalledWith('2'); // alpha -> beta
  });

  it('先頭で ArrowUp を押しても範囲外にならない', () => {
    const onSelect = vi.fn();
    render(
      <ProjectSwitcher
        projects={projects}
        activeProjectId="1"
        onSelect={onSelect}
        onClose={vi.fn()}
      />,
    );
    const input = screen.getByRole('combobox');
    fireEvent.keyDown(input, { key: 'ArrowUp' });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onSelect).toHaveBeenCalledWith('3'); // alpha のまま
  });

  // 末尾で ArrowDown を押しても最後の候補に留まる（下端のクランプ）。
  // 描画時にも index を clamp しているので、この 1 本だけでは onKeyDown 側の
  // Math.min(c + 1, matches.length - 1) を緩める変異が緑になる（実測）。次の 1 本が
  // その分を測る。
  it('末尾で ArrowDown を押しても範囲外にならない', () => {
    const onSelect = vi.fn();
    render(
      <ProjectSwitcher
        projects={projects}
        activeProjectId="1"
        onSelect={onSelect}
        onClose={vi.fn()}
      />,
    );
    const input = screen.getByRole('combobox');
    fireEvent.keyDown(input, { key: 'ArrowDown' });
    fireEvent.keyDown(input, { key: 'ArrowDown' });
    fireEvent.keyDown(input, { key: 'ArrowDown' });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onSelect).toHaveBeenCalledWith('1'); // alpha, beta, kamux の末尾 = kamux
  });

  // 末尾より下へ押し込んだ分がカーソルに溜まると、ArrowUp 1 回で戻る先がずれる。
  // onKeyDown 側のクランプ（Math.min(c + 1, matches.length - 1)）の観測点。
  it('末尾より下へ押し込んでも余分が溜まらない（ArrowUp 1 回で直前の候補へ戻る）', () => {
    const onSelect = vi.fn();
    render(
      <ProjectSwitcher
        projects={projects}
        activeProjectId="1"
        onSelect={onSelect}
        onClose={vi.fn()}
      />,
    );
    const input = screen.getByRole('combobox');
    fireEvent.keyDown(input, { key: 'ArrowDown' });
    fireEvent.keyDown(input, { key: 'ArrowDown' });
    fireEvent.keyDown(input, { key: 'ArrowDown' }); // 末尾（kamux）で頭打ち
    fireEvent.keyDown(input, { key: 'ArrowUp' });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onSelect).toHaveBeenCalledWith('2'); // alpha, beta, kamux の 2 番目 = beta
  });

  it('Escape で onClose が呼ばれる', () => {
    const onClose = vi.fn();
    render(
      <ProjectSwitcher
        projects={projects}
        activeProjectId="1"
        onSelect={vi.fn()}
        onClose={onClose}
      />,
    );
    fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Escape' });
    expect(onClose).toHaveBeenCalled();
  });

  it('候補が 0 件なら Enter で何も選ばない', () => {
    const onSelect = vi.fn();
    render(
      <ProjectSwitcher
        projects={projects}
        activeProjectId="1"
        onSelect={onSelect}
        onClose={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'zzzz' } });
    fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' });
    expect(onSelect).not.toHaveBeenCalled();
    expect(screen.getByText('該当するプロジェクトがありません')).toBeTruthy();
  });

  it('候補をクリックするとその id を選ぶ', () => {
    const onSelect = vi.fn();
    render(
      <ProjectSwitcher
        projects={projects}
        activeProjectId="1"
        onSelect={onSelect}
        onClose={vi.fn()}
      />,
    );
    fireEvent.click(screen.getAllByRole('option')[1]); // alpha, beta, kamux の 2 番目 = beta
    expect(onSelect).toHaveBeenCalledWith('2');
  });
});
