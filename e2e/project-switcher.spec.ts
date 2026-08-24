import { expect, test } from '@playwright/test';
import { tauriMockScript } from './support/tauriMock';

/**
 * `ProjectSwitcher.css`（M3-4 Task 12）が**実際に配線されている**ことを、実 CSS が
 * 解決される実ブラウザで固定する spec。
 *
 * jsdom は外部 CSS を解決しないので `ProjectSwitcher.test.tsx`（vitest）は
 * `import './ProjectSwitcher.css';` の 1 行が消えても緑のままになる。その状態では
 * `position: fixed` / `inset: 0` / `z-index` が失われ、スイッチャーはオーバーレイでは
 * なくページ内へインラインで流れ込む（先例 `e2e/archived-drawer.spec.ts`）。
 * **ここがその観測点。** vitest を緑のまま通す変異（import 1 行の削除）をこの spec が
 * 赤にすることを変異検証で確認した。
 */
test('Cmd+P のプロジェクトスイッチャーが実ブラウザで開き、position: fixed のオーバーレイに解決される（契約 §54.1）', async ({
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

  await page.keyboard.press('Meta+p');

  const dialog = page.getByRole('dialog', { name: 'プロジェクトを切り替え' });
  await expect(dialog).toBeVisible();
  // ProjectBar の <select> も option を持つのでダイアログ内に限定する。
  await expect(dialog.getByRole('option')).toHaveCount(2);

  // 本題。CSS が配線されていなければ scrim は既定の `static` になる。
  // E2E のためだけの data-testid は増やさないので、CSS のブロック名で引く。
  const scrimPosition = await page
    .locator('.project-switcher__scrim')
    .evaluate((el) => getComputedStyle(el).position);
  expect(scrimPosition).toBe('fixed');
});
