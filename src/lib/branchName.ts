/** 契約 §13 のブランチ名プレフィックス。 */
export const BRANCH_PREFIX = 'session/';

/** 契約 §13 の slug 切り詰め長。 */
export const SLUG_MAX_LENGTH = 40;

/**
 * 契約 §13 の slug 規則:
 * 小文字化 → 英数字とハイフン以外を '-' に → 連続ハイフン圧縮 → 前後ハイフン除去 → 40 文字切り詰め
 */
export function titleToSlug(title: string): string {
  return title
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, '-')
    .replace(/-{2,}/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, SLUG_MAX_LENGTH)
    .replace(/-+$/, '');
}

/**
 * タイトルからブランチ名を提案する。
 * slug が空になる場合（日本語のみのタイトル等）は null を返す。契約 §13 の fallback
 * "session-{id の先頭 8 文字}" は作成前の id を必要とするため、Rust 側の責務とする。
 */
export function proposeBranchName(title: string): string | null {
  const slug = titleToSlug(title);
  return slug === '' ? null : `${BRANCH_PREFIX}${slug}`;
}
