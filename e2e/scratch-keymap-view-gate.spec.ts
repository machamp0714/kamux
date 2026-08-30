import { expect, test, type Page } from '@playwright/test';
import { tauriMockScript } from './support/tauriMock';

/**
 * 契約 §11.4.2 / §97.2: Cmd+T（create_scratch_terminal）/ Cmd+W（close_scratch_terminal）は
 * `resolveTerminalOnlyAction`（src/hooks/keymap.ts:54, 呼び出しは :91 の
 * `ctx.view === 'terminal' ? resolveTerminalOnlyAction(e) : null`）を経由してしか
 * 発火しない。純関数側の view 条件は src/hooks/keymap.test.ts が kanban/editor の
 * ケースを既に持っているため、ここで足すのはそのユニットの重複ではなく
 * 「実ブラウザで kanban / editor 画面を表示したまま押しても、window の keydown
 * リスナ（useKeymap）から IPC まで飛ばない」という配線の継ぎ目だけである
 * （契約 §26.3: E2E はユニットテストの代替ではない）。
 *
 * scratch 端末が絡む IPC は 3 つ（src/store/sessionSlice.ts の
 * createScratchTerminal / closeScratchTerminal 経由。実 IPC 名は
 * src/ipc/commands.ts / src/store/sessionSlice.ts:259 の archiveSession の
 * 実装を実読して確認した —— archiveSession は専用コマンドではなく
 * updateSession(id, { archived_at }) = IPC `update_session` を呼ぶ）:
 *   - create_scratch_session（create_scratch_terminal）
 *   - stop_session / update_session（close_scratch_terminal: stopSession → archiveSession）
 * 「0 件」という assert は押鍵そのものが届いていなくても緑になるため、
 * 各テストに陽性対照（同じ経路で別のキーが実際に効くこと）を 1 つ置く。
 *
 * Cmd+W 側は `closeScratchTerminal`（src/store/sessionSlice.ts:515）が内部に
 * `is_scratch` ガードを持つ。`focusedSessionId` が指すセッションが
 * `is_scratch !== true` のままだと、view ゲート（resolveKeymap:91）を外しても
 * この内部ガードだけで発火が止まり、「0 件」が変化しないため view ゲート単独の
 * 効力を検証できない（修正ラウンド1 レビュー指摘）。そのため各テストでは
 * `focusedSessionId` を `is_scratch: true` の s2 へ向けてから kanban / editor
 * 画面へ戻り、Cmd+W が内部ガードを通過しうる状態で view ゲートだけを検証する。
 *
 * s2 へ focusedSessionId を向ける手段は kanban カードのクリックではない
 * ——is_scratch なセッションは契約 §29.4 によりカンバンに現れない
 * （`src/store/kanbanOrder.ts` が除外する）。かわりに terminal 画面の
 * `SessionTabList` は SCRATCH グループとして is_scratch セッションのタブを描く
 * （契約 §29.7 / `src/store/terminalSlice.ts` の `scratchTabs`）ので、s1 の
 * カードで terminal 画面に入ってから SCRATCH タブ（s2）をクリックして
 * `assignPane` 経由で focusedSessionId を s2 へ移す。
 */
function commonInitScript(): string {
  return tauriMockScript({
    commands: {
      list_projects: () => [
        { id: 'p1', name: 'kamux', repo_path: '/tmp/kamux', default_cli: 'claude' },
      ],
      list_sessions: () => [
        {
          id: 's1',
          project_id: 'p1',
          title: 'session one',
          description: '',
          kanban_status: 'in_progress',
          sort_order: 1000,
          mode: 'worktree',
          branch: 'session-one',
          worktree_path: null,
          cli_kind: 'claude',
          cli_command: null,
          claude_session_id: null,
          last_runtime_state: 'idle',
          last_runtime_error: null,
          first_started_at: null,
          archived_at: null,
          created_at: 0,
          updated_at: 0,
        },
        // is_scratch: true（契約 §29.2）。closeScratchTerminal の内部ガード
        // （sessionSlice.ts:515）を通過させ、Cmd+W 側で view ゲート単独の
        // 効力を検証できるようにするための固定具（修正ラウンド1 レビュー指摘）。
        {
          id: 's2',
          project_id: 'p1',
          title: 'scratch two',
          description: '',
          kanban_status: 'in_progress',
          sort_order: 2000,
          mode: 'worktree',
          branch: null,
          worktree_path: null,
          cli_kind: 'claude',
          cli_command: null,
          claude_session_id: null,
          last_runtime_state: 'idle',
          last_runtime_error: null,
          first_started_at: null,
          is_scratch: true,
          archived_at: null,
          created_at: 0,
          updated_at: 0,
        },
      ],
      start_session: (args) => ({
        id: args.id,
        project_id: 'p1',
        title: 'session one',
        description: '',
        kanban_status: 'in_progress',
        sort_order: 1000,
        mode: 'worktree',
        branch: 'session-one',
        worktree_path: null,
        cli_kind: 'claude',
        cli_command: null,
        claude_session_id: null,
        last_runtime_state: 'running',
        last_runtime_error: null,
        first_started_at: 0,
        archived_at: null,
        created_at: 0,
        updated_at: 0,
      }),
      // AppResult<String> = surface_id（契約 §7 / §19）。editor 画面到達だけに使う。
      spawn_editor: (args) => `${String(args.sessionId)}:editor`,
      write_pty: () => null,
      write_pty_bytes: () => null,
      resize_pty: () => null,
      ack_pty: () => null,
    },
  });
}

/** create_scratch_terminal / close_scratch_terminal が呼ぶ 3 コマンドだけを抜き出す。 */
function scratchCalls(calls: Array<{ cmd: string }>): Array<{ cmd: string }> {
  return calls.filter(
    (c) =>
      c.cmd === 'create_scratch_session' || c.cmd === 'stop_session' || c.cmd === 'update_session',
  );
}

async function kamuxCalls(page: Page): Promise<Array<{ cmd: string }>> {
  return page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
}

test.describe('Cmd+T / Cmd+W は terminal 画面限定（契約 §11.4.2 / §97.2 の配線）', () => {
  test('kanban 画面で押しても scratch 系 IPC は飛ばない（陽性対照: Cmd+2 は効く）', async ({
    page,
  }) => {
    await page.addInitScript(commonInitScript());
    await page.goto('/');
    await expect(page.locator('.kanban-view')).toBeVisible();
    const card = page.locator('.kanban-card[data-session-id="s1"]');
    await expect(card).toBeVisible();

    // s1 のカードで terminal 画面へ入り、SCRATCH タブ（s2）をクリックして
    // focusedSessionId を is_scratch な s2 へ向けてから kanban 画面へ戻る。
    // closeScratchTerminal の内部ガード（sessionSlice.ts:515）を通過できる状態に
    // しないと、Cmd+W 側で view ゲート単独の効力を検証できない（修正ラウンド1 指摘）。
    await card.click();
    await expect(page.locator('.kamux-terminal-view')).toBeVisible();
    await page.locator('.kamux-tab[data-session-id="s2"]').click();
    await page.keyboard.press('Meta+1');
    await expect(page.locator('.kanban-view')).toBeVisible();

    await page.keyboard.press('Meta+t');
    await page.keyboard.press('Meta+w');

    expect(scratchCalls(await kamuxCalls(page))).toHaveLength(0);

    // 陽性対照: 同じ window keydown 配線の別のキー（Cmd+2）が実際に効くことを確認する。
    // これが無いと上の「0 件」は Cmd+T / Cmd+W そのものが届いていない場合と
    // 区別が付かない。
    await page.keyboard.press('Meta+2');
    await expect(page.locator('.kamux-terminal-view')).toBeVisible();
  });

  test('editor 画面で押しても scratch 系 IPC は飛ばない（陽性対照: Cmd+1 は効く）', async ({
    page,
  }) => {
    await page.addInitScript(commonInitScript());
    await page.goto('/');
    const card = page.locator('.kanban-card[data-session-id="s1"]');
    await expect(card).toBeVisible();
    await card.click();
    await expect(page.locator('.kamux-terminal-view')).toBeVisible();

    // SCRATCH タブ（s2）をクリックして focusedSessionId を is_scratch な s2 へ
    // 向けてから editor 画面へ入る（理由は kanban 側のテストと同じ。上のコメント参照）。
    await page.locator('.kamux-tab[data-session-id="s2"]').click();
    await page.keyboard.press('Meta+3');
    await expect(page.locator('.kamux-editor-view')).toBeVisible();

    await page.keyboard.press('Meta+t');
    await page.keyboard.press('Meta+w');

    expect(scratchCalls(await kamuxCalls(page))).toHaveLength(0);

    // 陽性対照: Cmd+1 でカンバンへ戻れることを確認する。
    await page.keyboard.press('Meta+1');
    await expect(page.locator('.kanban-view')).toBeVisible();
  });
});
