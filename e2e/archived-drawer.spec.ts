import { expect, test } from '@playwright/test';
import { tauriMockScript } from './support/tauriMock';

/**
 * `ArchivedDrawer.css`（M3-4 Task 10）が**実際に配線されている**ことを、実 CSS が
 * 解決される実ブラウザで固定する spec。
 *
 * jsdom は外部 CSS を解決しないので `ArchivedDrawer.test.tsx`（vitest）は
 * `import './ArchivedDrawer.css';` の 1 行が消えても緑のままになる。その状態では
 * `position: fixed` / `inset: 0` / `z-index` が失われ、ドロワーはオーバーレイではなく
 * ページ内へインラインで流れ込む —— 契約 §126 が記録した `interrupted-overlay*` の
 * 再発と同型（先例 `cleanup-worktree.spec.ts`）。**ここがその観測点。**
 *
 * `e2e/kanban.spec.ts` に足さない理由: あちらの `beforeEach` は 2 セッションとも
 * `backlog` / `archived_at: null` の共通フィクスチャを持ち、アーカイブ済みの前提と
 * 噛み合わない。共通フィクスチャを広げると既存テストの前提が本題と無関係に動く
 * （Task 9 が同じ判断をしている。裁定 64）。
 */
test('アーカイブ済みドロワーが実ブラウザで開き、position: fixed のオーバーレイに解決される（契約 §54.1 / 裁定 63）', async ({
  page,
}) => {
  await page.addInitScript(
    tauriMockScript({
      commands: {
        list_projects: () => [
          { id: 'p1', name: 'kamux', repo_path: '/tmp/kamux', default_cli: 'claude' },
        ],
        list_sessions: () => [
          {
            id: 's1',
            project_id: 'p1',
            title: 'archived task',
            description: '',
            kanban_status: 'done',
            sort_order: 1000,
            cli_kind: 'claude',
            mode: 'in_place',
            branch: null,
            worktree_path: null,
            archived_at: 1754006400000,
            last_runtime_state: 'exited',
            last_runtime_error: null,
            first_started_at: 1000,
          },
        ],
      },
    }),
  );
  await page.goto('/');

  await page.getByRole('button', { name: 'アーカイブ済み' }).click();

  const drawer = page.getByRole('complementary', { name: 'アーカイブ済み' });
  await expect(drawer).toBeVisible();
  await expect(page.getByText('archived task')).toBeVisible();

  // 本題。CSS が配線されていなければ scrim は既定の `static` になる。
  // E2E のためだけの data-testid は増やさないので、CSS のブロック名で引く。
  const scrimPosition = await page
    .locator('.archived-drawer__scrim')
    .evaluate((el) => getComputedStyle(el).position);
  expect(scrimPosition).toBe('fixed');
});
