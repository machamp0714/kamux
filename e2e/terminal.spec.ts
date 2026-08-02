import { expect, test, type Page } from '@playwright/test';
import { tauriMockScript } from './support/tauriMock';

/**
 * 契約 §5: surface_id は `${sessionId}:${kind}`。TerminalPane は 'agent' サーフェスを使う。
 */
function agentSurface(sessionId: string): string {
  return `${sessionId}:agent`;
}

/** Buffer に依存せず（@types/node 未導入）、バイト列を base64 化する。 */
function bytesToBase64(bytes: Uint8Array): string {
  let binary = '';
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary);
}

/**
 * 共通の 1 プロジェクト・2 セッション（両方 in_progress = ターミナルのタブに両方出る。
 * selectTerminalTabs は TAB_COLUMN_ORDER の先頭が in_progress）。
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
        {
          id: 's2',
          project_id: 'p1',
          title: 'session two',
          description: '',
          kanban_status: 'in_progress',
          sort_order: 2000,
          mode: 'worktree',
          branch: 'session-two',
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
      ],
      // AppResult<Session>。id 以外はテストで使わないので固定値でよい。
      start_session: (args) => ({
        id: args.id,
        project_id: 'p1',
        title: 'session',
        description: '',
        kanban_status: 'in_progress',
        sort_order: 1000,
        mode: 'worktree',
        branch: 'session',
        worktree_path: null,
        cli_kind: 'claude',
        cli_command: null,
        claude_session_id: null,
        last_runtime_state: 'running',
        last_runtime_error: null,
        first_started_at: Date.now(),
        archived_at: null,
        created_at: 0,
        updated_at: 0,
      }),
      write_pty: () => null,
      write_pty_bytes: () => null,
      resize_pty: () => null,
      ack_pty: () => null,
    },
  });
}

async function gotoTerminalView(page: Page) {
  await page.goto('/');
  await expect(page.locator('[data-session-id="s1"]')).toBeVisible();
  await page.keyboard.press('Meta+2');
  await expect(page.locator('.kamux-terminal-view')).toBeVisible();
}

test.describe('ターミナル画面（Cmd+2 到達 + xterm 配線）', () => {
  test('Cmd+2 でターミナル画面に切り替わる', async ({ page }) => {
    await page.addInitScript(commonInitScript());
    await page.goto('/');
    await expect(page.locator('[data-session-id="s1"]')).toBeVisible();
    await expect(page.locator('.kamux-terminal-view')).toHaveCount(0);

    await page.keyboard.press('Meta+2');

    await expect(page.locator('.kamux-terminal-view')).toBeVisible();
    await expect(page.locator('.kanban-view')).toHaveCount(0);
  });

  test('xterm にフォーカスがある状態で Cmd+J / Cmd+K がタブ移動として効き、write_pty は飛ばない', async ({
    page,
  }) => {
    await page.addInitScript(commonInitScript());
    await gotoTerminalView(page);

    // s1 タブをクリックしてアクティブにする（TerminalPane が attachTerminal → term.focus() する）
    await page.locator('[data-session-id="s1"]').click();
    await expect(page.locator('[data-session-id="s1"]')).toHaveAttribute('aria-selected', 'true');

    // xterm の textarea に実際にフォーカスが移っていることを確認してから押す
    // （ここが「ユニットテストの守備範囲外」と申告した継ぎ目そのもの）。
    await expect(page.locator('.xterm-helper-textarea')).toBeFocused();

    await page.keyboard.press('Meta+j');
    await expect(page.locator('[data-session-id="s2"]')).toHaveAttribute('aria-selected', 'true');

    await page.keyboard.press('Meta+k');
    await expect(page.locator('[data-session-id="s1"]')).toHaveAttribute('aria-selected', 'true');

    const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
    expect(calls.filter((c) => c.cmd === 'write_pty' || c.cmd === 'write_pty_bytes')).toHaveLength(
      0,
    );
  });

  test('タブをクリックすると start_session が飛ぶ。Cmd+1→Cmd+2 の再マウントでも起動済みなら再送しない（StrictMode 二重発火を検出できる形）', async ({
    page,
  }) => {
    await page.addInitScript(commonInitScript());
    await gotoTerminalView(page);

    const startCallCountFor = async (id: string) => {
      const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
      return calls.filter((c) => c.cmd === 'start_session' && (c.args as { id: string }).id === id)
        .length;
    };

    await page.locator('[data-session-id="s1"]').click();
    await expect.poll(() => startCallCountFor('s1')).toBe(1);
    // ここまでは sessionId が null → 's1' への通常の依存変化（React はここで
    // effect を StrictMode 二重実行しない）。start_session の引数も確認しておく。
    const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
    expect(calls.filter((c) => c.cmd === 'start_session')[0].args).toMatchObject({ id: 's1' });

    // Kanban へ切り替えて TerminalView ごと unmount し、Cmd+2 で戻す。
    // App.tsx は view === 'terminal' のときしか <TerminalView /> を描画しないため、
    // これが TerminalPane にとって「本当の初回マウント」になる。paneAssignment は
    // ストアに残ったままなので、この再マウントは sessionId='s1' を最初から持つ
    // ——React 18 StrictMode が effect を 2 回実行するのはまさにこの瞬間であり、
    // 依存配列変化による通常の再実行（上のタブクリック）では 2 回実行されない。
    await page.keyboard.press('Meta+1');
    await expect(page.locator('.kanban-view')).toBeVisible();
    await page.keyboard.press('Meta+2');
    await expect(page.locator('.kamux-terminal-view')).toBeVisible();

    // isStarted が立ったままなので、再マウントしても start_session は再送されない。
    // .poll() で「たまたま 1 を通過した瞬間」を拾わないよう、実時間で一呼吸置いてから
    // 1 回だけスナップショットする（早期一致による見逃しを避ける）。
    await page.waitForTimeout(1000);
    expect(await startCallCountFor('s1')).toBe(1);
  });

  /**
   * xterm.js のレンダラは headless Chromium では WebGL が選ばれる（着手前に実測。
   * `.xterm-screen canvas` のクラス無し要素が `getContext('webgl2') !== null` を返した）。
   * `screenReaderMode`（`.xterm-accessibility-tree` を生成する唯一のフラグ）は
   * `registry.ts` の Terminal 生成時オプションに含まれておらず、ここは触れない
   * （§16 の所有者は M1-3、今回のタスクでは src/ 変更が禁止）。
   * そのため「描画された文字列」を DOM からは読めない。
   *
   * ここでは「2 チャンクが xterm まで届き、両方消化されたこと」を
   * `ack_pty(seq=2)` の呼び出しで見る（AckCoalescer は最後に consumed した seq だけを送る。
   * ack_coalescer.test.ts が既に単体で担保している規約）。
   * **UTF-8 のマルチバイト境界をまたいでも文字化けしないことまではこの assert は守らない**
   * （不正なバイト列でも term.write は例外を投げず replacement char になるだけで、
   * ack は同じく飛ぶ）。境界の正しさの目視確認は手動スモークへ回す。
   */
  test('pty://data の 2 チャンクが xterm まで届き、ack_pty(seq=2) が返る', async ({ page }) => {
    // fix round 1 の e2e 着手中に発見した実バグにより現状 fixme（詳細はレビュー報告）:
    // src/terminal/ackCoalescer.ts の `schedule: (fn) => void = queueMicrotask` は
    // 呼び出し側で `this.schedule(fn)`（メソッド呼び出し構文）として呼ばれるため、
    // ネイティブの `queueMicrotask` に AckCoalescer インスタンスが `this` として渡り、
    // 実ブラウザ（Chromium 実測 / WKWebView も同じ制約を持つ native API）では
    // `TypeError: Illegal invocation` を投げる。ackCoalescer.test.ts は常に自前の
    // schedule 関数を注入しており、既定値の queueMicrotask 経路を一度も通していない
    // ため単体テストでは検出されない。src/ の修正は今回のタスク範囲外（BLOCKED として報告済み）。
    // 修正後にこの行を外せば、このテストは本来の意図どおり検証として機能する。
    test.fixme(
      true,
      'ackCoalescer.ts の queueMicrotask this-binding バグにより ack_pty が飛ばない',
    );
    await page.addInitScript(commonInitScript());
    await gotoTerminalView(page);
    await page.locator('[data-session-id="s1"]').click();

    await expect
      .poll(async () => {
        const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
        return calls.filter((c) => c.cmd === 'start_session').length;
      })
      .toBe(1);

    const surface = agentSurface('s1');
    const text = 'hello 日本語';
    const bytes = new TextEncoder().encode(text);
    // マルチバイト文字（日本語の先頭）の途中で 2 分割する経路自体は残す
    // （UTF-8 の再構成コードパスを通すため。正しさの検証は上記コメントの通り手動）。
    const splitAt = 7;
    const chunk1 = bytesToBase64(bytes.slice(0, splitAt));
    const chunk2 = bytesToBase64(bytes.slice(splitAt));

    await page.evaluate(
      ([eventName, base64, seq]) => {
        window.__TAURI_INTERNALS__.__kamuxEmit(eventName, { base64, seq });
      },
      [`pty://data/${surface}`, chunk1, 1] as [string, string, number],
    );
    await page.evaluate(
      ([eventName, base64, seq]) => {
        window.__TAURI_INTERNALS__.__kamuxEmit(eventName, { base64, seq });
      },
      [`pty://data/${surface}`, chunk2, 2] as [string, string, number],
    );

    await expect
      .poll(async () => {
        const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
        return calls.filter((c) => c.cmd === 'ack_pty').map((c) => (c.args as { seq: number }).seq);
      })
      .toEqual([2]);
  });

  /**
   * `[process exited: 0]` の文字列表示自体は上記と同じ理由で DOM から読めないため手動送り。
   * ここでは `pty://exit` ハンドラの副作用（`ptyBridge.ts` の `startedSurfaces.delete`）を、
   * 「exit 後にタブを離れて戻ると start_session がもう一度飛ぶ」という
   * 観測可能な形で見る（`TerminalPane` は `isStarted(surface)` が false のときだけ再起動する）。
   */
  test('pty://exit の後にタブを離れて戻ると start_session が再び飛ぶ（起動済みフラグが落ちる）', async ({
    page,
  }) => {
    await page.addInitScript(commonInitScript());
    await gotoTerminalView(page);
    await page.locator('[data-session-id="s1"]').click();

    const startCallCountFor = async (id: string) => {
      const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
      return calls.filter((c) => c.cmd === 'start_session' && (c.args as { id: string }).id === id)
        .length;
    };

    await expect.poll(() => startCallCountFor('s1')).toBe(1);

    const surface = agentSurface('s1');
    await page.evaluate(
      ([eventName, payload]) => {
        window.__TAURI_INTERNALS__.__kamuxEmit(eventName, payload);
      },
      [`pty://exit/${surface}`, { surface_id: surface, exit_code: 0 }] as [
        string,
        { surface_id: string; exit_code: number },
      ],
    );

    // 別タブへ切り替えてから s1 へ戻る（TerminalPane の sessionId prop を変えて再マウントさせる）
    await page.locator('[data-session-id="s2"]').click();
    await page.locator('[data-session-id="s1"]').click();

    await expect.poll(() => startCallCountFor('s1')).toBe(2);
  });
});

/**
 * start_session の reject は kanban.spec.ts の move_session 失敗ケースと同じ理由で
 * 独立 describe の外に置く（addInitScript は複数回張ると評価順が保証されないため 1 本だけ）。
 *
 * 赤字エラー表示（writeNotice の tone: 'error'）の文字列自体は DOM から読めないため手動送り。
 * ここでは `unmarkStarted(surface)`（TerminalPane の reject 分岐）の副作用を、
 * 「失敗後にタブを離れて戻ると start_session が再試行される」ことで見る。
 */
test('start_session が reject した後、タブを離れて戻ると再試行される（起動済みフラグが戻る）', async ({
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
          {
            id: 's2',
            project_id: 'p1',
            title: 'session two',
            description: '',
            kanban_status: 'in_progress',
            sort_order: 2000,
            mode: 'worktree',
            branch: 'session-two',
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
        ],
        start_session: () => {
          throw { code: 'pty_spawn', message: 'forced' };
        },
      },
    }),
  );
  await page.goto('/');
  await expect(page.locator('[data-session-id="s1"]')).toBeVisible();
  await page.keyboard.press('Meta+2');
  await expect(page.locator('.kamux-terminal-view')).toBeVisible();

  await page.locator('[data-session-id="s1"]').click();

  const startCallCountFor = async (id: string) => {
    const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
    return calls.filter((c) => c.cmd === 'start_session' && (c.args as { id: string }).id === id)
      .length;
  };

  await expect.poll(() => startCallCountFor('s1')).toBe(1);

  await page.locator('[data-session-id="s2"]').click();
  await page.locator('[data-session-id="s1"]').click();

  await expect.poll(() => startCallCountFor('s1')).toBe(2);
});
