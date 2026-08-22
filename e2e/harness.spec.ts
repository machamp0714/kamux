import { expect, test } from '@playwright/test';
import { tauriMockScript } from './support/tauriMock';

/**
 * `tauriMockScript()` の既定値上書きに対する観測（M2-3 Task 16 修正ラウンド 1）。
 *
 * `e2e/support/tauriMock.ts` の `defaultHandlers` は
 * `{ ...defaultHandlers, ${entries} }` の順でオブジェクトを組み立てており、
 * spec.commands に同名キーがあれば下のスプレッドで defaultHandlers を上書きする設計
 * である。この spec は、その上書きが実際に効くことを、アプリの画面（バナー等）を経由
 * せず、ハーネス自身（`window.__TAURI_INTERNALS__.invoke` の戻り値）で直接確かめる。
 *
 * `set_visibility_context` は既定・上書きのどちらも `undefined` を返すため、戻り値
 * では 2 状態を区別できない。この観測対象には含めない。
 */

function mockScript(): string {
  return tauriMockScript({
    commands: {
      list_projects: () => [],
      list_sessions: () => [],
      notification_permission: () => 'denied',
    },
  });
}

test('notification_permission は spec 側の宣言で defaultHandlers を上書きする', async ({
  page,
}) => {
  await page.addInitScript(mockScript());
  await page.goto('/');

  const result = await page.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke('notification_permission'),
  );

  expect(result).toBe('denied');
});
