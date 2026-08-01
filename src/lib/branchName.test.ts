import { describe, expect, it } from 'vitest';
import { BRANCH_PREFIX, proposeBranchName, titleToSlug } from './branchName';

describe('titleToSlug', () => {
  it('小文字化し、英数字とハイフン以外をハイフンにする', () => {
    expect(titleToSlug('Fix Login Bug')).toBe('fix-login-bug');
  });

  it('連続したハイフンを 1 つに圧縮する', () => {
    expect(titleToSlug('Fix   Login!!! Bug')).toBe('fix-login-bug');
  });

  it('前後のハイフンを除去する', () => {
    expect(titleToSlug('  --Fix Login--  ')).toBe('fix-login');
  });

  it('既存のハイフンは保つ', () => {
    expect(titleToSlug('re-run tests')).toBe('re-run-tests');
  });

  it('40 文字に切り詰める', () => {
    // 契約 §13 の 40 をリテラルで固定する（SLUG_MAX_LENGTH の自己参照にしない）
    expect(titleToSlug('a'.repeat(50))).toHaveLength(40);
  });

  it('切り詰めで残った末尾ハイフンも除去する', () => {
    // 'a' を 39 個（index 0〜38）+ 空白（index 39 でハイフンに変換）+ 'zzzz'。
    // slice(0, 40) は index 0〜39 を含むため、ハイフンがちょうど切り詰め境界の
    // 末尾（40 文字目）に残る。この末尾ハイフンが .replace(/-+$/, '') で
    // 除去されることを確認する。
    expect(titleToSlug(`${'a'.repeat(39)} zzzz`)).toBe('a'.repeat(39));
  });

  it('英数字を含まない入力では空文字を返す', () => {
    expect(titleToSlug('日本語タイトル')).toBe('');
    expect(titleToSlug('!!!')).toBe('');
    expect(titleToSlug('')).toBe('');
  });

  it('絵文字のみでは空文字を返す', () => {
    expect(titleToSlug('🎉')).toBe('');
  });

  it('空白のみでは空文字を返す', () => {
    expect(titleToSlug('   ')).toBe('');
  });

  it('絵文字を含む混在入力では絵文字を除去する', () => {
    expect(titleToSlug('Fix 🎉 bug')).toBe('fix-bug');
  });
});

describe('proposeBranchName', () => {
  it('契約 §13 のプレフィックスを付けて返す', () => {
    expect(BRANCH_PREFIX).toBe('session/');
    expect(proposeBranchName('Fix Login Bug')).toBe('session/fix-login-bug');
  });

  it('slug が空になる場合は null を返す（id を持つ Rust 側の fallback に委ねる）', () => {
    expect(proposeBranchName('日本語タイトル')).toBeNull();
    expect(proposeBranchName('')).toBeNull();
  });
});

describe('契約 §51.3.3 共有テストベクタ', () => {
  // 00-contracts.md §51.3.3（4596-4614 行）の表と 1:1 対応させる。
  // M1-4 の Rust 側（worktree::slug::title_slug）も同じ表を実装する契約であり、
  // 食い違い調査のとき突合できるよう row を契約の行番号のまま残す。
  // 固定するのは「入出力」であり手順ではない（§51.3.3 が明言）。
  const vectors: Array<{
    row: number;
    label: string;
    input: string;
    slug: string;
    branch: string | null;
  }> = [
    {
      row: 1,
      label: 'Fix Login Bug',
      input: 'Fix Login Bug',
      slug: 'fix-login-bug',
      branch: 'session/fix-login-bug',
    },
    {
      row: 2,
      label: '  Fix  ---  Login!! ',
      input: '  Fix  ---  Login!! ',
      slug: 'fix-login',
      branch: 'session/fix-login',
    },
    {
      row: 3,
      label: 're-run tests',
      input: 're-run tests',
      slug: 're-run-tests',
      branch: 'session/re-run-tests',
    },
    {
      row: 4,
      label: 'Fix issue 1234',
      input: 'Fix issue 1234',
      slug: 'fix-issue-1234',
      branch: 'session/fix-issue-1234',
    },
    {
      row: 5,
      label: '日本語 fix login',
      input: '日本語 fix login',
      slug: 'fix-login',
      branch: 'session/fix-login',
    },
    {
      row: 6,
      label: "'a' × 50",
      input: 'a'.repeat(50),
      slug: 'a'.repeat(40),
      branch: `session/${'a'.repeat(40)}`,
    },
    {
      row: 7,
      // slug 化後は 'b'*40 + '-c'（42 文字）。slice(0, 40) がちょうど 'b'*40 で
      // 止まり、末尾にハイフンが残らない。行 6（ハイフン以外で切り詰め）とも、
      // titleToSlug の「切り詰めで残った末尾ハイフンも除去する」テスト
      // （'a'.repeat(39)+' zzzz'、末尾にハイフンが残る）とも別の境界。
      label: "'b' × 40 + ' c'",
      input: `${'b'.repeat(40)} c`,
      slug: 'b'.repeat(40),
      branch: `session/${'b'.repeat(40)}`,
    },
    {
      row: 8,
      // 行 8・9 は契約が明記する唯一の意図的な非対称（§51.3.3）。
      // Rust 側は作成前の id を持つため "session-{id 先頭 8 文字}" を返すが、
      // TS 側にはその id が無いため '' / null を返して Rust の fallback に
      // 委ねる。これは食い違いではない。TS に "session-" fallback を足さないこと。
      label: 'ログイン不具合の修正',
      input: 'ログイン不具合の修正',
      slug: '',
      branch: null,
    },
    { row: 9, label: "'!!!'", input: '!!!', slug: '', branch: null },
    { row: 9, label: "''", input: '', slug: '', branch: null },
  ];

  it.each(vectors)('#$row $label', ({ input, slug, branch }) => {
    expect(titleToSlug(input)).toBe(slug);
    expect(proposeBranchName(input)).toBe(branch);
  });
});
