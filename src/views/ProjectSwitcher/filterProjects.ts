import type { Project } from '../../types/model';

/** name マッチが常に repo_path マッチより上位に来るようにするための固定ペナルティ */
const PATH_PENALTY = 1000;

/**
 * 大文字小文字を無視したサブシーケンス一致。**小さいほど良い**スコアを返す。
 * スコア = 最後にマッチした位置 - (needle の長さ - 1)。
 * マッチしなければ null。
 */
export function fuzzyScore(haystack: string, needle: string): number | null {
  if (needle === '') return 0;
  const h = haystack.toLowerCase();
  const n = needle.toLowerCase();
  let cursor = 0;
  let last = -1;
  for (let i = 0; i < n.length; i += 1) {
    const found = h.indexOf(n[i], cursor);
    if (found === -1) return null;
    last = found;
    cursor = found + 1;
  }
  return last - (n.length - 1);
}

/** クエリにマッチするプロジェクトを、決定的な順序で返す */
export function filterProjects(projects: Project[], query: string): Project[] {
  const q = query.trim();
  const scored: Array<{ project: Project; score: number }> = [];
  for (const project of projects) {
    const byName = fuzzyScore(project.name, q);
    const byPath = fuzzyScore(project.repo_path, q);
    if (byName === null && byPath === null) continue;
    const score = byName !== null ? byName : (byPath as number) + PATH_PENALTY;
    scored.push({ project, score });
  }
  scored.sort((a, b) => a.score - b.score || a.project.name.localeCompare(b.project.name));
  return scored.map((x) => x.project);
}
