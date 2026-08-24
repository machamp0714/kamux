import { describe, expect, it } from 'vitest';
import type { Project } from '../../types/model';
import { filterProjects, fuzzyScore } from './filterProjects';

const p = (id: string, name: string, repoPath: string): Project => ({
  id,
  name,
  repo_path: repoPath,
  default_cli: 'claude',
  created_at: 0,
  updated_at: 0,
});

describe('fuzzyScore', () => {
  it('空クエリは 0', () => {
    expect(fuzzyScore('kamux', '')).toBe(0);
  });

  it('先頭からの連続一致は 0', () => {
    expect(fuzzyScore('kamux', 'kam')).toBe(0);
  });

  it('サブシーケンスにマッチする', () => {
    // k(0) m(2) x(4): 先頭位置 0 + ギャップ (4-0-2)=2
    expect(fuzzyScore('kamux', 'kmx')).toBe(2);
  });

  it('後方の一致はスコアが大きい（＝順位が下がる）', () => {
    expect(fuzzyScore('my-kamux', 'kam')).toBe(3);
  });

  it('大文字小文字を無視する', () => {
    expect(fuzzyScore('KaMuX', 'kmx')).toBe(2);
  });

  it('マッチしなければ null', () => {
    expect(fuzzyScore('kamux', 'xyz')).toBeNull();
  });

  it('順序が違えばマッチしない', () => {
    expect(fuzzyScore('kamux', 'xk')).toBeNull();
  });

  it('繰り返し文字は連続一致しない（カーソルは一致ごとに進む）', () => {
    // 'kamux' に 'k' は 1 個しか無いので 'kk' はマッチしない
    expect(fuzzyScore('kamux', 'kk')).toBeNull();
  });
});

describe('filterProjects', () => {
  const projects = [
    p('1', 'kamux', '/Users/me/repo/kamux'),
    p('2', 'beta', '/Users/me/work/kamux-docs'),
    p('3', 'alpha', '/Users/me/repo/alpha'),
  ];

  it('空クエリでは全件を名前順で返す', () => {
    expect(filterProjects(projects, '').map((x) => x.name)).toEqual(['alpha', 'beta', 'kamux']);
  });

  it('名前一致はパス一致より上位に来る', () => {
    expect(filterProjects(projects, 'kamux').map((x) => x.id)).toEqual(['1', '2']);
  });

  it('サブシーケンスで絞り込める', () => {
    expect(filterProjects(projects, 'kmx').map((x) => x.id)).toEqual(['1', '2']);
  });

  it('パスだけでも引ける', () => {
    expect(filterProjects(projects, 'work').map((x) => x.id)).toEqual(['2']);
  });

  it('どこにもマッチしない候補は落とす', () => {
    expect(filterProjects(projects, 'zzzz')).toEqual([]);
  });

  it('前後の空白を無視する', () => {
    expect(filterProjects(projects, '  alpha  ').map((x) => x.id)).toEqual(['3']);
  });

  it('パス一致は name 一致より必ず下位に来る（後方の name 一致でも先頭の path 一致より上位）', () => {
    // lateName: byName スコア 3（後方一致）。earlyPath: byPath スコア 1（先頭一致、name は無マッチ）。
    // PATH_PENALTY が効いていれば、byPath にペナルティが乗るため lateName が必ず先に来る。
    const lateName = p('L', 'my-user', '/opt/late');
    const earlyPath = p('E', 'zzz', '/Users/x');
    expect(filterProjects([earlyPath, lateName], 'user').map((x) => x.id)).toEqual(['L', 'E']);
  });
});
