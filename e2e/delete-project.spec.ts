import { expect, test } from '@playwright/test';
import { tauriMockScript } from './support/tauriMock';

/**
 * `DeleteProjectDialog.css`（M3-4 Task 12）が**実際に配線されている**ことを、実 CSS が
 * 解決される実ブラウザで固定する spec。
 *
 * jsdom は外部 CSS を解決しないので `DeleteProjectDialog.test.tsx`（vitest）は
 * `import './DeleteProjectDialog.css';` の 1 行が消えても緑のままになる。その状態では
 * `position: fixed` / `inset: 0` / `z-index` が失われ、確認ダイアログはオーバーレイでは
 * なくページ内へインラインで流れ込む（先例 `e2e/project-switcher.spec.ts` /
 * `e2e/cleanup-worktree.spec.ts`）。**ここがその観測点。**
 */
test('プロジェクト削除の確認ダイアログが実ブラウザで開き、backdrop が position: fixed のオーバーレイに解決される（契約 §54.1 / §130.3）', async ({
  page,
}) => {
  await page.addInitScript(
    tauriMockScript({
      commands: {
        list_projects: () => [
          { id: 'p1', name: 'kamux', repo_path: '/tmp/kamux', default_cli: 'claude' },
          { id: 'p2', name: 'beta', repo_path: '/tmp/beta', default_cli: 'claude' },
        ],
        list_sessions: () => [],
      },
    }),
  );
  await page.goto('/');

  // 契約 §130.3: 削除導線は ProjectBar（管理面）に在る。ProjectSwitcher には無い。
  // E2E のためだけの data-testid は増やさないので、既存の aria-label で引く。
  await page.getByRole('button', { name: 'kamux を削除' }).click();

  const dialog = page.getByRole('dialog', { name: 'プロジェクトを削除' });
  await expect(dialog).toBeVisible();
  // 契約 §130.4: worktree は残ることを 1 行出す。
  await expect(dialog).toContainText('作業ツリーは残ります');

  // 本題。CSS が配線されていなければ backdrop は既定の `static` になる。
  const backdropPosition = await page
    .locator('.delete-project-dialog__backdrop')
    .evaluate((el) => getComputedStyle(el).position);
  expect(backdropPosition).toBe('fixed');
});
