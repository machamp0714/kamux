import { expect, test, type Page } from '@playwright/test';
import { tauriMockScript } from './support/tauriMock';

/**
 * PointerSensor の activationConstraint: { distance: 5 }（第1部 判断 7 / sensors.ts）により、
 * down -> up だけではドラッグが開始しない。5px 以上の move が要る。
 * 契約 §49.8 が「変更しない」と定めているのは move_session の発火アサーションであって、
 * このマウス操作の手順ではない。
 */
async function dragCardTo(page: Page, fromSessionId: string, target: { x: number; y: number }) {
  const card = page.locator(`[data-session-id="${fromSessionId}"]`);
  const box = await card.boundingBox();
  if (box === null) throw new Error(`card ${fromSessionId} has no bounding box`);
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + 20, box.y + box.height / 2 + 20, { steps: 5 });
  await page.mouse.move(target.x, target.y, { steps: 10 });
  await page.mouse.up();
}

test.describe('カンバン操作（共通の 1 プロジェクト・2 セッション）', () => {
  test.beforeEach(async ({ page }) => {
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
              title: 'fix login',
              description: '',
              kanban_status: 'backlog',
              sort_order: 1000,
              cli_kind: 'claude',
              mode: 'worktree',
              branch: 'fix-login',
              archived_at: null,
            },
            {
              id: 's2',
              project_id: 'p1',
              title: 'add tests',
              description: '',
              kanban_status: 'backlog',
              sort_order: 2000,
              cli_kind: 'claude',
              mode: 'worktree',
              branch: 'add-tests',
              archived_at: null,
            },
          ],
          // 契約 §49.4: move_session は移動先の列の Session[] を sort_order 昇順で返す。
          // 楽観更新後に sessionOrder[to] をこの戻り値で丸ごと置き換える（sessionSlice.moveCard）ため、
          // 配列の順序がそのまま画面上の表示順になる。
          move_session: (args) => {
            if (args.toStatus === 'in_progress') {
              return [
                {
                  id: 's1',
                  project_id: 'p1',
                  title: 'fix login',
                  description: '',
                  kanban_status: 'in_progress',
                  sort_order: 1000,
                  cli_kind: 'claude',
                  mode: 'worktree',
                  branch: 'fix-login',
                  archived_at: null,
                },
              ];
            }
            // 同一列（backlog）内の入れ替え: s1 を s2 の後ろへ
            return [
              {
                id: 's2',
                project_id: 'p1',
                title: 'add tests',
                description: '',
                kanban_status: 'backlog',
                sort_order: 1500,
                cli_kind: 'claude',
                mode: 'worktree',
                branch: 'add-tests',
                archived_at: null,
              },
              {
                id: 's1',
                project_id: 'p1',
                title: 'fix login',
                description: '',
                kanban_status: 'backlog',
                sort_order: 2500,
                cli_kind: 'claude',
                mode: 'worktree',
                branch: 'fix-login',
                archived_at: null,
              },
            ];
          },
        },
      }),
    );
    await page.goto('/');
    await expect(page.locator('[data-session-id="s1"]')).toBeVisible();

    // 第1部 §9「表示」: 4 列見出しの件数、カードの CLI アイコン・ブランチ名表示、
    // runtime バッジが 1 枚も出ないこと（M1-2 は runtimeStates が空のまま。契約 §34.7）。
    await expect(page.locator('section[aria-label="Backlog"] .kanban-column__count')).toHaveText(
      '2',
    );
    await expect(
      page.locator('section[aria-label="In Progress"] .kanban-column__count'),
    ).toHaveText('0');
    await expect(page.locator('[data-column="backlog"] .kanban-card__cli')).toHaveCount(2);
    await expect(page.locator('[data-column="backlog"] .kanban-card__branch')).toHaveCount(2);
    await expect(page.locator('.kanban-card__badge')).toHaveCount(0);
  });

  test('カードを Backlog から In Progress へドラッグすると move_session が飛び、盤面に反映される', async ({
    page,
  }) => {
    const target = await page.locator('[data-column="in_progress"]').boundingBox();
    if (target === null) throw new Error('column has no bounding box');
    await dragCardTo(page, 's1', {
      x: target.x + target.width / 2,
      y: target.y + target.height / 2,
    });

    const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
    expect(calls.filter((c) => c.cmd === 'move_session')).toHaveLength(1);

    // 楽観更新だけでなく、モックの戻り値が実際に適用されたことを見る
    // （move_session がプレーンオブジェクトを返すなど契約違反の形でも呼び出し履歴だけは
    //  残ってしまうため、DOM 側の反映まで確認しないと壊れた実装を見逃す）。
    await expect(page.locator('[data-column="in_progress"] [data-session-id="s1"]')).toBeVisible();
    await expect(page.locator('[data-column="backlog"] [data-session-id="s1"]')).toHaveCount(0);
    await expect(page.locator('.error-toast')).toHaveCount(0);
  });

  test('同じ列内でカードを入れ替えると move_session が飛び、並びが入れ替わる', async ({ page }) => {
    const target = await page.locator('[data-session-id="s2"]').boundingBox();
    if (target === null) throw new Error('card s2 has no bounding box');
    await dragCardTo(page, 's1', {
      x: target.x + target.width / 2,
      y: target.y + target.height * 0.75,
    });

    const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
    const moveSessionCalls = calls.filter((c) => c.cmd === 'move_session');
    expect(moveSessionCalls).toHaveLength(1);
    expect(moveSessionCalls[0].args).toMatchObject({ id: 's1', toStatus: 'backlog' });

    // 主アサーションはモックの戻り値どおりに並びが入れ替わったこと
    // （dnd-kit の over 解決の細部より、盤面に反映される最終結果の方が壊れにくい）。
    await expect(page.locator('[data-column="backlog"] .kanban-card__title')).toHaveText([
      'add tests',
      'fix login',
    ]);
    await expect(page.locator('.error-toast')).toHaveCount(0);
  });

  test('Cmd+N でモーダルが開き、Escape で閉じる', async ({ page }) => {
    const dialog = page.getByRole('dialog', { name: '新規セッション' });
    await expect(dialog).toBeHidden();

    await page.keyboard.press('Meta+n');
    await expect(dialog).toBeVisible();

    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
  });

  test('Cmd+1 でカンバン画面が表示される', async ({ page }) => {
    const board = page.locator('.kanban-view');
    await expect(board).toBeVisible();

    // M1-2 の時点ではカンバン以外へ切り替える UI/キー操作が無いため、
    // ここで検証できるのは「Cmd+1 を押してもカンバンが表示されたままである」こと
    // （M1-3 以降が Cmd+2 を足したら、そちらから戻るケースを追加する）。
    await page.keyboard.press('Meta+1');
    await expect(board).toBeVisible();
    await expect(page.locator('.app__placeholder')).toHaveCount(0);
  });

  test('move_session が失敗すると .error-toast が表示され、カードは元の列に戻る', async ({
    page,
  }) => {
    // move_session だけ失敗させるため、このテストは共通モックを上書きする。
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
              title: 'fix login',
              description: '',
              kanban_status: 'backlog',
              sort_order: 1000,
              cli_kind: 'claude',
              mode: 'worktree',
              branch: 'fix-login',
              archived_at: null,
            },
          ],
          move_session: () => {
            throw { code: 'db', message: 'forced' };
          },
        },
      }),
    );
    await page.reload();
    await expect(page.locator('[data-session-id="s1"]')).toBeVisible();

    const target = await page.locator('[data-column="in_progress"]').boundingBox();
    if (target === null) throw new Error('column has no bounding box');
    await dragCardTo(page, 's1', {
      x: target.x + target.width / 2,
      y: target.y + target.height / 2,
    });

    const toast = page.locator('.error-toast');
    await expect(toast).toBeVisible();
    await expect(page.locator('.error-toast__code')).toHaveText('db');
    await expect(page.locator('.error-toast__message')).toHaveText('forced');

    // ロールバック: カードは Backlog に留まる（sessionSlice.moveCard の catch 節）
    await expect(page.locator('[data-column="backlog"] [data-session-id="s1"]')).toBeVisible();
    await expect(page.locator('[data-column="in_progress"] [data-session-id="s1"]')).toHaveCount(0);

    await page.locator('.error-toast__close').click();
    await expect(toast).toBeHidden();
  });
});

test('起動時復元: activeProjectId の選択・sort_order 順の描画・不正 ID のフォールバックが UI に反映される', async ({
  page,
}) => {
  // localStorage が既に値を持つ場合（後段の reload）は上書きしない。
  // Playwright は複数 addInitScript の評価順を保証しないため、後から足すスクリプトで
  // 「後勝ち」を狙う設計にはしない（本スクリプトを毎回 no-op 化できる形にする）。
  await page.addInitScript(() => {
    if (window.localStorage.getItem('kamux.activeProjectId') === null) {
      window.localStorage.setItem('kamux.activeProjectId', 'p2');
    }
  });
  await page.addInitScript(
    tauriMockScript({
      commands: {
        list_projects: () => [
          { id: 'p1', name: 'proj-1', repo_path: '/tmp/p1', default_cli: 'claude' },
          { id: 'p2', name: 'proj-2', repo_path: '/tmp/p2', default_cli: 'claude' },
        ],
        list_sessions: (args) => {
          if (args.projectId !== 'p2') return [];
          // sort_order は昇順に並んでいない状態で返す。表示側で並べ替わることを見る。
          return [
            {
              id: 's-c',
              project_id: 'p2',
              title: 'C',
              description: '',
              kanban_status: 'backlog',
              sort_order: 3000,
              cli_kind: 'claude',
              mode: 'in_place',
              branch: null,
              archived_at: null,
            },
            {
              id: 's-a',
              project_id: 'p2',
              title: 'A',
              description: '',
              kanban_status: 'backlog',
              sort_order: 1000,
              cli_kind: 'claude',
              mode: 'in_place',
              branch: null,
              archived_at: null,
            },
            {
              id: 's-b',
              project_id: 'p2',
              title: 'B',
              description: '',
              kanban_status: 'in_progress',
              sort_order: 2000,
              cli_kind: 'claude',
              mode: 'in_place',
              branch: null,
              archived_at: null,
            },
          ];
        },
      },
    }),
  );

  await page.goto('/');

  // localStorage の kamux.activeProjectId が指す 2 件目 (p2) が選択状態
  await expect(page.getByRole('button', { name: 'proj-2' })).toHaveClass(
    /project-bar__item--active/,
  );

  // sort_order 昇順どおりに、それぞれ正しい列へ描画される
  await expect(page.locator('[data-column="backlog"] .kanban-card__title')).toHaveText(['A', 'C']);
  await expect(page.locator('[data-column="in_progress"] .kanban-card__title')).toHaveText(['B']);

  // localStorage が存在しないプロジェクトを指す状態にしてから再読み込みする
  await page.evaluate(() => {
    window.localStorage.setItem('kamux.activeProjectId', 'p-missing');
  });
  await page.reload();

  // 先頭のプロジェクト (p1) に落ちて選択状態になり、盤面が描かれる
  await expect(page.getByRole('button', { name: 'proj-1' })).toHaveClass(
    /project-bar__item--active/,
  );
  await expect(page.locator('.kanban-view')).toBeVisible();
});
