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
 * `set_visibility_context` はこの観測対象に含めない。理由は推測ではなく試作で確かめた:
 * spec 側で `set_visibility_context: () => undefined` を明示宣言した場合も、宣言しない
 * （defaultHandlers の既定のみが効く）場合も、`invoke('set_visibility_context')` の戻り値は
 * 共に `undefined` だった（`AppResult<()>` に対応する仕様なので当然ではあるが、実測して
 * から確認した）。戻り値だけでは 2 状態を区別できないため、この形の観測は足さない。
 * `defaultHandlers` の全メンバ数は 2（`set_visibility_context` / `notification_permission`）
 * で、戻り値だけで上書きを観測できるのはこのうち `notification_permission` の 1 件のみ。
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
