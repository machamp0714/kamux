import { expect, test } from '@playwright/test';
import { tauriMockScript } from './support/tauriMock';

/**
 * `CleanupWorktreeDialog.css`（M3-4 Task 9）が**実際に配線されている**ことを、実 CSS が
 * 解決される実ブラウザで固定する spec。
 *
 * jsdom は外部 CSS を解決しないので `CleanupWorktreeDialog.test.tsx`（vitest）は
 * `import './CleanupWorktreeDialog.css';` の 1 行が消えても緑のままになる。その状態では
 * `position: fixed` / `inset: 0` / `z-index` が失われ、ダイアログはオーバーレイではなく
 * ページ内へインラインで流れ込む —— 契約 §126 が記録した `interrupted-overlay*` の
 * 再発そのものである。**ここがその観測点。**
 *
 * `e2e/kanban.spec.ts` に足さずに独立したファイルにした理由:
 * あちらの `test.describe` は 1 プロジェクト・2 セッション（どちらも `backlog`）の共通
 * `beforeEach` を持ち、掃除導線の前提（`kanban_status: 'done'` + `mode: 'worktree'` +
 * `worktree_path` 非 null。`store/cleanup.ts` の `isCleanupSuggested`）とも
 * `worktree_status` コマンドのモックとも噛み合わない。共通フィクスチャを掃除向けに
 * 広げると、あちらの既存 12 テストの前提が本題と無関係に動く。
 */
test('worktree 掃除ダイアログが実ブラウザで開き、backdrop が position: fixed のオーバーレイに解決される（契約 §54.1 / 裁定 48）', async ({
  page,
}) => {
  await page.addInitScript(
    tauriMockScript({
      commands: {
        list_projects: () => [
          { id: 'p1', name: 'kamux', repo_path: '/tmp/kamux', default_cli: 'claude' },
        ],
        // isCleanupSuggested(session) が true になる形（done + worktree + worktree_path 非 null）。
        // どれか 1 つでも欠けると KanbanCardCleanup が何も描かず、下の click が届かない。
        list_sessions: () => [
          {
            id: 's1',
            project_id: 'p1',
            title: 'fix login',
            description: '',
            kanban_status: 'done',
            sort_order: 1000,
            cli_kind: 'claude',
            mode: 'worktree',
            branch: 'session/fix-login',
            worktree_path: '/tmp/kamux/.worktrees/session-fix-login',
            archived_at: null,
            last_runtime_state: 'exited',
            last_runtime_error: null,
            first_started_at: 1000,
          },
        ],
        worktree_status: () => ({ dirty: false, entries: [] }),
      },
    }),
  );
  await page.goto('/');
  await expect(page.locator('[data-session-id="s1"]')).toBeVisible();

  // `.kanban-card__actions` は既定で `opacity: 0; pointer-events: none`（kanban.css）。
  // hover しないと click が pointer-events に弾かれて test timeout まで待たされる。
  await page.locator('[data-session-id="s1"].kanban-card').hover();
  await page.locator('[aria-label="worktree を掃除"]').click();

  const dialog = page.getByRole('dialog', { name: 'worktree を掃除' });
  await expect(dialog).toBeVisible();

  // 本題。CSS が配線されていなければ backdrop は既定の `static` になる。
  // E2E のための data-testid は足さない（契約 §26 の方針）ので、CSS のブロック名で引く。
  const backdropPosition = await page
    .locator('.cleanup-worktree-dialog__backdrop')
    .evaluate((el) => getComputedStyle(el).position);
  expect(backdropPosition).toBe('fixed');
});
