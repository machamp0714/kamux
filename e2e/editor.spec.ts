import { expect, test, type Page } from '@playwright/test';
import { tauriMockScript } from './support/tauriMock';

/**
 * M3-1 Task 7 の E2E。**DOM とライブラリと実 CSS を跨ぐ経路だけ**を置く
 * （契約 §26.3: E2E はユニットテストの代替ではない。純関数に切り出せるものは vitest 側）。
 *
 *  1. モーダル表示中はエディタへ打鍵が飛ばない（契約 §16 の modal === null 規則）
 *  2. モーダルを閉じた後は飛ぶ（同規則の「modal の遷移に追従する」側）
 *  3. EditorView.css が実ブラウザで解決され、xterm のコンテナがビューを埋める
 *     （契約 §76.6。fix round 1 で追加）
 *  4. 手動スモーク 33-a: spawn_editor が CliNotFound で reject したとき、
 *     ガイド付きエラーが描画されアプリが落ちない（契約 §80.2 の層 2）
 *
 * 再起動経路（pty://exit → 再起動オーバーレイ）はここでは踏まない。
 * `pty://exit/{sid}` には ptyBridge と EditorSurface の 2 者が購読しており、
 * 合成 emit と解除の順序・id の対応がモック実装の都合に依存する。
 * 再起動は EditorView.test.tsx（vitest）側で守る。
 */

/** 契約 §5: surface_id は `${sessionId}:${kind}`。エディタ画面は 'editor' サーフェス。 */
function editorSurface(sessionId: string): string {
  return `${sessionId}:editor`;
}

/**
 * `spawn_editor` は契約 §7 のとおり `AppResult<String>`（= surface_id）を返す。
 * 戻り値の形を契約と合わせておかないと E2E は緑になるが何も検証しない（契約 §55.5）。
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
      // AppResult<String> = surface_id（契約 §7 / §19。冪等なので同じ値を返す）
      spawn_editor: (args) => `${String(args.sessionId)}:editor`,
      write_pty: () => null,
      write_pty_bytes: () => null,
      resize_pty: () => null,
      ack_pty: () => null,
    },
  });
}

/** カードをクリックして（focusedSessionId を確定させて）から Cmd+3 でエディタ画面へ。 */
async function gotoEditorView(page: Page) {
  await page.goto('/');
  const card = page.locator('.kanban-card[data-session-id="s1"]');
  await expect(card).toBeVisible();
  await card.click();
  await expect(page.locator('.kamux-terminal-view')).toBeVisible();

  await page.keyboard.press('Meta+3');
  await expect(page.locator('.kamux-editor-view')).toBeVisible();
}

async function calls(page: Page) {
  return page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
}

test.describe('エディタ画面（Cmd+3 到達 + フォーカスの段）', () => {
  /**
   * 契約 §16 / §11.4.6: `attachTerminal` はフォーカスを当てない。当てるのは
   * `EditorSurface` であり、`modal === null` のときだけである。ここでは
   * 「モーダルが可視のまま打鍵しても nvim の PTY へ流れない」ことを固定する。
   * 変異（modal !== null のガードを外す）で赤くなることを実測した。
   */
  test('モーダル表示中に Cmd+3 でエディタへ入っても、打鍵が nvim へ流れない', async ({ page }) => {
    await page.addInitScript(commonInitScript());
    await gotoEditorView(page);

    await expect
      .poll(async () => (await calls(page)).filter((c) => c.cmd === 'spawn_editor').length)
      .toBe(1);

    // Cmd+N はエディタ画面からも効く（契約 §11.4.2）。openModal の副作用で view は
    // kanban へ切り替わるので、この時点で EditorView は一旦 unmount される
    await page.keyboard.press('Meta+n');
    await expect(page.locator('[role="dialog"]')).toBeVisible();

    // モーダルは view 分岐の外にあるため残ったまま、EditorSurface だけが再マウントされる
    await page.keyboard.press('Meta+3');
    await expect(page.locator('.kamux-editor-view')).toBeVisible();
    await expect(page.locator('[role="dialog"]')).toBeVisible();

    await page.keyboard.type('x');

    const after = await calls(page);
    expect(after.filter((c) => c.cmd === 'write_pty' || c.cmd === 'write_pty_bytes')).toHaveLength(
      0,
    );
    // 宛先が正しくモーダルのタイトル入力へ届いていることも直接確認する
    await expect(page.locator('[role="dialog"] input[type="text"]').first()).toHaveValue('x');
  });

  /**
   * フォーカスは modal の遷移に追従する必要がある（契約 §16 の 3 点目）。
   * 「マウント時に 1 度評価する」だけだと、モーダルを閉じた後にエディタが無フォーカスの
   * まま残り、nvim が一切操作できなくなる。変異（フォーカス effect の依存配列から
   * modal を外す）で赤くなることを実測した。
   */
  test('モーダルを閉じると、再マウントなしでエディタへの打鍵が nvim へ届く', async ({ page }) => {
    await page.addInitScript(commonInitScript());
    await gotoEditorView(page);

    await page.keyboard.press('Meta+n');
    await expect(page.locator('[role="dialog"]')).toBeVisible();
    await page.keyboard.press('Meta+3');
    await expect(page.locator('.kamux-editor-view')).toBeVisible();

    // Escape はモーダル表示中に close_modal として効く（契約 §11）。
    // view は変わらないので EditorSurface は再マウントされない
    await page.keyboard.press('Escape');
    await expect(page.locator('[role="dialog"]')).toHaveCount(0);

    await page.keyboard.type('x');

    await expect
      .poll(
        async () =>
          (await calls(page)).filter(
            (c) =>
              c.cmd === 'write_pty' &&
              (c.args as { surfaceId: string }).surfaceId === editorSurface('s1'),
          ).length,
      )
      .toBe(1);
  });

  /**
   * `EditorView.css` が実ブラウザで解決されていることを見る（契約 §76.6: 「実 CSS が
   * 解決されること」は E2E で自動化する。Vite dev server + 実 CSS + chromium）。
   *
   * **`height > 0` では守れない。** xterm は自前の CSS を持つので、`EditorView.css` を
   * 外しても `.editor-surface` は xterm の内容分の高さ（既定 80x24 ぶん）を持つ
   * ——「高さがある」は「レイアウトが効いている」ではない（実測: 665 → 288 に退化しても
   * `> 0` は緑のまま）。
   *
   * **viewport 依存の絶対値は固定しない**（次にレイアウトを触った人に理由の分からない
   * 赤を出さないため）。代わりに、このスタイルシートだけが与える 2 つの関係を見る:
   *   - `.kamux-editor-view` が `position: relative`（オーバーレイの包含ブロック）
   *   - `.editor-surface` が `position: absolute` + `inset: 0` の結果として**親を埋める**
   */
  test('EditorView.css が解決され、xterm のコンテナがビューを埋める', async ({ page }) => {
    await page.addInitScript(commonInitScript());
    await gotoEditorView(page);

    const view = page.locator('.kamux-editor-view');
    const surface = page.locator('.editor-surface');

    // オーバーレイ（終了・エラー・上限）の絶対配置はこの包含ブロックに依存する
    await expect(view).toHaveCSS('position', 'relative');
    await expect(surface).toHaveCSS('position', 'absolute');

    const viewBox = await view.boundingBox();
    const surfaceBox = await surface.boundingBox();
    // 退化の検出用。ヘッダ 1 本ぶんより明らかに大きいことだけを要求する
    expect(viewBox?.height ?? 0).toBeGreaterThan(100);
    // inset: 0 の帰結。親の高さ / 幅と一致する（絶対値ではなく関係を見ている）
    expect(surfaceBox?.height ?? 0).toBeCloseTo(viewBox?.height ?? -1, 0);
    expect(surfaceBox?.width ?? 0).toBeCloseTo(viewBox?.width ?? -1, 0);
  });
});

/**
 * 手動スモーク項目 33-a（契約 §80.2 の層 2）。`spawn_editor` が `CliNotFound` で
 * reject したとき、エディタ画面が**ガイド付きエラーを描画し、アプリが落ちない**こと。
 * モックの reject 値は契約 §6 の `AppError { code, message }`（code は snake_case）。
 *
 * addInitScript は 1 spec につき 1 回だけ張る（Playwright は複数スクリプトの評価順を
 * 保証しない。kanban.spec.ts / terminal.spec.ts と同じ理由で独立した test にする）。
 */
test('spawn_editor が cli_not_found で reject すると、ガイド付きエラーが出てアプリは落ちない', async ({
  page,
}) => {
  const pageErrors: string[] = [];
  page.on('pageerror', (e) => pageErrors.push(e.message));

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
        // 契約 §6: AppError は { code, message }。code は snake_case（§2）
        spawn_editor: () => {
          throw {
            code: 'cli_not_found',
            message:
              '`nvim` が見つかりませんでした。\n検索したディレクトリ:\n  /usr/bin\n  /opt/homebrew/bin\n\nすでにインストール済みの場合は、ターミナルで `which nvim` を実行してください。',
          };
        },
        write_pty: () => null,
        write_pty_bytes: () => null,
        resize_pty: () => null,
        ack_pty: () => null,
      },
    }),
  );

  await page.goto('/');
  const card = page.locator('.kanban-card[data-session-id="s1"]');
  await expect(card).toBeVisible();
  await card.click();
  await page.keyboard.press('Meta+3');
  await expect(page.locator('.kamux-editor-view')).toBeVisible();

  // ガイド付きエラー: 何が起きたかと、原文（検索したディレクトリを含む）と、やり直す手段
  const overlay = page.locator('.editor-overlay');
  await expect(overlay).toBeVisible();
  await expect(overlay).toContainText('nvim を起動できませんでした');
  await expect(overlay.locator('.editor-overlay__detail')).toContainText('検索したディレクトリ');
  await expect(overlay.getByRole('button', { name: '再試行' })).toBeVisible();
  // 上限エラーの案内（枠を空ける導線）と取り違えていないこと
  await expect(overlay).not.toContainText(':qa');

  // アプリが落ちていない: 例外が投げられておらず、画面遷移も生きている
  expect(pageErrors).toEqual([]);
  await page.keyboard.press('Meta+1');
  await expect(page.locator('.kanban-view')).toBeVisible();
});
