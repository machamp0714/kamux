import { expect, test, type Locator, type Page } from '@playwright/test';
import { tauriMockScript } from './support/tauriMock';

/**
 * M3-3（汎用 CLI）の「推定状態」表示を、**実 CSS が解決された実ブラウザ**で固定する spec。
 *
 * 契約 §76.6（RULINGS §14.6）: 手動スモークの項目 1 / 9 / 10 は中空ドットと `~` 前置を
 * 目視条件にしていたが、実 CSS の解決は §26.4 のどのカテゴリにも当たらないため手動から
 * 追い出された。**その観測の受け皿がこのファイルである**（書かれないと、この観測は
 * どこにも属さないまま消える）。
 *
 * `RuntimeBadge.test.tsx`（vitest）との住み分け:
 * jsdom は外部 CSS を解決しないため、あちらはインライン style の `var(--state-*)` 参照と
 * `data-estimated` / ラベル文字列までしか固定できない。ここで見るのは
 * **`.runtime-badge--estimated .runtime-badge__dot` が実際に中空へ解決されること**
 * （background が透明・枠が var(--state-*) の実色・border-box で 8×8 が保たれること）である。
 * ラベルの `~` 前置だけは実 CSS の話ではないが、中空ドットと対になった 1 つの視覚条件
 * （components.md「実行状態バッジ」節。8×8 の円だけでは視認性が弱いのでテキストにも合図を持たせる）
 * として手動スモークが目視していたため、その処分を引き取る意味でここでも見る。
 *
 * 意図的な設計: `s-silence`（idle + silence_timeout）と `s-hook`（idle + hook_stop）は
 * **同じ状態・同じ色トークン**で、推定かどうかだけが違う。塗りつぶし側を同じ page に
 * 同居させないと「常に中空」が検出できず、中空ドットが何も意味していない状態と区別が付かない。
 */

/** 契約 §53.4 の色トークン。`waiting_input` だけ `--state-waiting`（RuntimeBadge.tsx の RUNTIME_STATE_TOKEN）。 */
const STATE_IDLE_TOKEN = '--state-idle';
const STATE_WAITING_TOKEN = '--state-waiting';

/**
 * 3 セッションとも `last_runtime_state` を持たせて（= `first_started_at` が非 null）
 * バッジを描かせる。`seedRuntimeStates` は `first_started_at === null` を seed から外すので、
 * null にするとバッジ自体が描かれず、両テストが無関係な理由で赤くなる（契約 §34.6）。
 * `reason` は `list_sessions` からは入らない（`sessionSlice.ts` の seed はむしろ
 * `runtimeReasons` を `{}` へ戻す）ので、推定/確定は `session://state` で駆動する。
 */
function mockScript(): string {
  return tauriMockScript({
    commands: {
      list_projects: () => [
        { id: 'p1', name: 'kamux', repo_path: '/tmp/kamux', default_cli: 'shell' },
      ],
      // ブラウザ側で toString() 経由で再評価されるため、外側の変数を参照しないこと
      list_sessions: () => [
        {
          id: 's-bel',
          project_id: 'p1',
          title: 'bel',
          description: '',
          kanban_status: 'backlog',
          sort_order: 1000,
          mode: 'in_place',
          branch: null,
          worktree_path: null,
          cli_kind: 'shell',
          cli_command: 'npm run dev',
          claude_session_id: null,
          last_runtime_state: 'waiting_input',
          last_runtime_error: null,
          first_started_at: 1000,
          heuristics_enabled: true,
          silence_timeout_secs: 30,
          archived_at: null,
        },
        {
          id: 's-silence',
          project_id: 'p1',
          title: 'silence',
          description: '',
          kanban_status: 'backlog',
          sort_order: 2000,
          mode: 'in_place',
          branch: null,
          worktree_path: null,
          cli_kind: 'shell',
          cli_command: 'npm run dev',
          claude_session_id: null,
          last_runtime_state: 'idle',
          last_runtime_error: null,
          first_started_at: 1000,
          heuristics_enabled: true,
          silence_timeout_secs: 30,
          archived_at: null,
        },
        {
          id: 's-hook',
          project_id: 'p1',
          title: 'hook',
          description: '',
          kanban_status: 'backlog',
          sort_order: 3000,
          mode: 'in_place',
          branch: null,
          worktree_path: null,
          cli_kind: 'claude',
          cli_command: null,
          claude_session_id: null,
          last_runtime_state: 'idle',
          last_runtime_error: null,
          first_started_at: 1000,
          heuristics_enabled: false,
          silence_timeout_secs: 30,
          archived_at: null,
        },
      ],
    },
  });
}

function badgeOf(page: Page, sessionId: string): Locator {
  return page.locator(`[data-session-id="${sessionId}"] .runtime-badge`);
}

/**
 * `session://state/{session_id}`（契約 §8）を合成 emit して、バッジが期待の推定/確定へ
 * 落ち着くまで打ち直す。
 *
 * 「ハンドラが登録されたか」を待たずに outcome で待つ理由: `tauriMock.ts` の解除は配列を
 * 詰めず no-op 関数を差し込むので、`__kamuxEventHandlers[ev].length > 0` は解除後も真になる。
 * StrictMode の mount→unmount→mount を通ると「no-op のスロットだけが居て、生きた listen は
 * まだ pending」の窓が実在し、そこで 1 度だけ emit すると無音で落ちて、赤の原因が
 * 変異なのかタイミングなのか切り分けられなくなる。`applyStateEvent` は同値なら新しい
 * オブジェクトを作らない（`sessionSlice.ts`）ので、打ち直しは冪等である。
 *
 * イベント名の suffix と `payload.session_id` は同じ `string` で取り違えても読んで
 * 気づかない（別キーへ書かれてバッジが変わらないだけ）ため、**1 個の引数から両方を組む**。
 */
async function emitStateUntil(
  page: Page,
  args: { sessionId: string; runtimeState: string; reason: string },
  expectedEstimated: 'true' | 'false',
): Promise<void> {
  const badge = badgeOf(page, args.sessionId);
  await expect
    .poll(async () => {
      await page.evaluate(({ sessionId, runtimeState, reason }) => {
        window.__TAURI_INTERNALS__.__kamuxEmit(`session://state/${sessionId}`, {
          session_id: sessionId,
          runtime_state: runtimeState,
          reason,
        });
      }, args);
      return badge.getAttribute('data-estimated');
    })
    .toBe(expectedEstimated);
}

/**
 * `var(--state-*)` を直接参照した probe 要素の解決結果。特定の色値をハードコードしないので
 * テーマが変わっても通る（`kanban.spec.ts` の runtime バッジ spec と同じ手法）。
 */
async function resolvedTokenColor(page: Page, token: string): Promise<string> {
  return page.evaluate((t) => {
    const probe = document.createElement('div');
    probe.style.color = `var(${t})`;
    document.body.appendChild(probe);
    const value = getComputedStyle(probe).color;
    probe.remove();
    return value;
  }, token);
}

interface DotMetrics {
  backgroundColor: string;
  borderColor: string;
  borderStyle: string;
  borderWidth: string;
  width: number;
  height: number;
}

/** ドットの実 CSS 解決結果。寸法は border-box を見たいので getBoundingClientRect を使う
 *  （getComputedStyle().width は content box を返すため、box-sizing の脱落を見逃す）。 */
async function dotMetrics(badge: Locator): Promise<DotMetrics> {
  return badge.locator('.runtime-badge__dot').evaluate((el) => {
    const cs = getComputedStyle(el);
    const rect = el.getBoundingClientRect();
    return {
      backgroundColor: cs.backgroundColor,
      borderColor: cs.borderColor,
      borderStyle: cs.borderStyle,
      borderWidth: cs.borderWidth,
      width: rect.width,
      height: rect.height,
    };
  });
}

test('ヒューリスティック由来の推定状態は実 CSS で中空ドットに解決され、ラベルに ~ が付く（契約 §76.6）', async ({
  page,
}) => {
  await page.addInitScript(mockScript());
  await page.goto('/');
  await expect(badgeOf(page, 's-bel')).toBeVisible();

  // BEL 検知 → waiting_input（src-tauri/src/session/runtime_state.rs）
  await emitStateUntil(
    page,
    { sessionId: 's-bel', runtimeState: 'waiting_input', reason: 'bel_detected' },
    'true',
  );
  // 沈黙判定 → idle（同上）
  await emitStateUntil(
    page,
    { sessionId: 's-silence', runtimeState: 'idle', reason: 'silence_timeout' },
    'true',
  );

  const cases = [
    { sessionId: 's-bel', token: STATE_WAITING_TOKEN, label: '~入力待ち' },
    { sessionId: 's-silence', token: STATE_IDLE_TOKEN, label: '~アイドル' },
  ];

  for (const { sessionId, token, label } of cases) {
    const badge = badgeOf(page, sessionId);
    await expect(badge).toHaveClass(/runtime-badge--estimated/);
    await expect(badge.locator('.runtime-badge__label')).toHaveText(label);

    const dot = await dotMetrics(badge);
    const expectedColor = await resolvedTokenColor(page, token);

    // 中空: 塗りつぶしが消え、枠が var(--state-*) の実色で立つ
    expect(dot.backgroundColor).toBe('rgba(0, 0, 0, 0)');
    expect(dot.borderStyle).toBe('solid');
    // CSS の指定値は 1.5px だが、getComputedStyle が返すのは used value で、Chromium は
    // deviceScaleFactor=1（playwright.config.ts は指定していないので既定の 1）で
    // 枠幅を整数 px へ丸める。実測値のリテラルで固定する ——「> 0」のような
    // 何でも通る境界は書かない（契約 §81）。
    expect(dot.borderWidth).toBe('1px');
    // 未解決の var() は継承チェーンへ落ちて「一見もっともらしい」別の rgb() を返すので、
    // 空文字/透明かどうかではなく probe の解決結果と一致することを見る
    expect(dot.borderColor).toMatch(/^rgb\(\d+, \d+, \d+\)$/);
    expect(dot.borderColor).toBe(expectedColor);
    // box-sizing: border-box が効いていて、枠を足しても 8×8 のまま
    expect(dot.width).toBe(8);
    expect(dot.height).toBe(8);
  }
});

test('権威ある reason（hook_stop）のバッジは中空にならず ~ も付かない（契約 §76.6）', async ({
  page,
}) => {
  await page.addInitScript(mockScript());
  await page.goto('/');
  const badge = badgeOf(page, 's-hook');
  await expect(badge).toBeVisible();

  // 先に推定側へ振ってから確定側へ戻す。こうしないと「イベントが届いていない」状態と
  // 「確定 reason が届いて塗りつぶしのまま」が区別できない
  // （hook_stop のバッジは reason 未着のバッジと見た目が同じである）。
  await emitStateUntil(
    page,
    { sessionId: 's-hook', runtimeState: 'idle', reason: 'silence_timeout' },
    'true',
  );
  await emitStateUntil(
    page,
    { sessionId: 's-hook', runtimeState: 'idle', reason: 'hook_stop' },
    'false',
  );

  await expect(badge).not.toHaveClass(/runtime-badge--estimated/);
  // toHaveText は完全一致なので、`~アイドル` になれば赤くなる
  await expect(badge.locator('.runtime-badge__label')).toHaveText('アイドル');

  const dot = await dotMetrics(badge);
  const expectedColor = await resolvedTokenColor(page, STATE_IDLE_TOKEN);

  // 塗りつぶし: background が var(--state-idle) の実色で、枠は立たない
  expect(dot.backgroundColor).toMatch(/^rgb\(\d+, \d+, \d+\)$/);
  expect(dot.backgroundColor).toBe(expectedColor);
  expect(dot.borderWidth).toBe('0px');
  expect(dot.width).toBe(8);
  expect(dot.height).toBe(8);
});
