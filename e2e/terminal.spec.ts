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

    // s1 タブをクリックしてアクティブにする（PR 10 fix round 1 以降、フォーカスは
    // TerminalPane の modal === null 用 effect が当てる。attachTerminal 自体はもう focus しない）
    await page.locator('[data-session-id="s1"]').click();
    await expect(page.locator('[data-session-id="s1"]')).toHaveAttribute('aria-selected', 'true');

    // xterm の textarea に実際にフォーカスが移っていることを確認してから押す
    // （ここが「ユニットテストの守備範囲外」と申告した継ぎ目そのもの）。
    await expect(page.locator('.xterm-helper-textarea')).toBeFocused();

    await page.keyboard.press('Meta+j');
    await expect(page.locator('[data-session-id="s2"]')).toHaveAttribute('aria-selected', 'true');

    await page.keyboard.press('Meta+k');
    await expect(page.locator('[data-session-id="s1"]')).toHaveAttribute('aria-selected', 'true');

    // タブ移動が実際に効いたこと（上の aria-selected の遷移）はここまでで証明済み
    // ——xterm がフォーカスを持つ間も window の keydown リスナへ Cmd 系が届く、という
    // 契約 §11 のバブリングの継ぎ目そのものを踏んでいる。
    //
    // 一方、以下の write_pty 0 件のアサーションは fix round 2 で変異実測したところ
    // 恒真の疑いが強い: registry.ts の attachCustomKeyEventHandler を
    // `() => true`（xterm にキーを処理させる）へ変異させても、この assert は赤くならない。
    // headless Chromium では Meta+j がそもそも xterm 内でキー列（PTY へ書く文字列）を
    // 生まない模様で、この engine ではこの assert に弁別力が無い。
    // 「Cmd+J を押してシェルに j が入らないこと」の実測は §26.4 の手動スモーク
    // （WKWebView）側で確認する。ここでは「write_pty が呼ばれていないこと」という
    // 現状の事実だけを回帰の下限として残す。
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

    // 実測済み（fix round 2）: ackCoalescer.ts 修正後も toEqual([2]) は赤くなる。
    // 2 回の emit は別々の page.evaluate（CDP 往復を挟む別タスク）なので、chunk1 の
    // ack（seq=1）が chunk2 の emit より先に飛び、実際の列は [1, 2] になる。
    // 「最後の ack が seq=2」だけを見れば、この非決定な順序に依存せず、
    // ack が一切来ない本来の赤（本バグ再発時）は引き続き検出できる。
    await expect
      .poll(async () => {
        const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
        const acks = calls
          .filter((c) => c.cmd === 'ack_pty')
          .map((c) => (c.args as { seq: number }).seq);
        return acks[acks.length - 1];
      })
      .toBe(2);
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

  /**
   * 契約 §16 が名指しする危険（onData の二重登録 = 打鍵 1 回が 2 回 PTY に届く）を、
   * `registry.test.ts:183`（同一 surface への attachTerminal 複数回呼び出し）や
   * `ptyBridge.test.ts:48/55`（同一 surface への ensurePtySubscription 多重呼び出し）
   * とは異なる経路で踏む。それらは「同一 surface への重複呼び出し」を守っているが、
   * ここで踏むのは「ペイン再割当（タブ切り替え）を跨いだ再マウント」——
   * `TerminalPane` の sessionId 依存変化のたびに cleanup（detachTerminal のみ。
   * disposeTerminal も disposePtySubscription も呼ばない）→ 再実行（attachTerminal）
   * するが、registry.ts の entries / onData 登録は生きたままになる経路である。
   */
  test('タブを A→B→A→B→A と往復してから合成キー入力すると write_pty が 1 回だけ飛ぶ（onData の二重登録防止）', async ({
    page,
  }) => {
    await page.addInitScript(commonInitScript());
    await gotoTerminalView(page);

    await page.locator('[data-session-id="s1"]').click();
    await expect
      .poll(async () => {
        const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
        return calls.filter(
          (c) => c.cmd === 'start_session' && (c.args as { id: string }).id === 's1',
        ).length;
      })
      .toBe(1);

    // A→B→A→B→A（2 往復）
    await page.locator('[data-session-id="s2"]').click();
    await page.locator('[data-session-id="s1"]').click();
    await page.locator('[data-session-id="s2"]').click();
    await page.locator('[data-session-id="s1"]').click();

    await expect(page.locator('[data-session-id="s1"]')).toHaveAttribute('aria-selected', 'true');
    // detachTerminal は host を display:none にするだけで DOM から取り除かない
    // （契約 §16: スクロールバックを保ったまま付け替える）。そのため s1 / s2 両方の
    // .xterm-helper-textarea が同じコンテナに共存する。加えて xterm の textarea は
    // 意図的に極小サイズ（visible な要素として bounding box を持たない）なので
    // Playwright の :visible 疑似クラスでは絞り込めない。display:none でない
    // .kamux-term-host にスコープして絞り込む。
    await expect(
      page.locator('.kamux-term-host:not([style*="display: none"]) .xterm-helper-textarea'),
    ).toBeFocused();

    // Cmd 修飾なしの通常キー。優先度 4 で確認した「Meta+j は Chromium ではキー列を
    // 生まない」問題には当たらない（恒真にならない）。
    await page.keyboard.type('x');

    await expect
      .poll(async () => {
        const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
        return calls.filter(
          (c) =>
            c.cmd === 'write_pty' && (c.args as { surfaceId: string }).surfaceId === 's1:agent',
        ).length;
      })
      .toBe(1);
  });

  /**
   * PR 10 fix round 1（Critical）。registry.ts の attachTerminal は term.focus() を
   * 呼んでいた（契約 §16 に無い副作用）。SessionFormModal は view 分岐の外にあるため
   * unmount されず、Cmd+N でモーダルを開いたまま Cmd+2 でターミナルへ戻ると
   * TerminalPane が再マウントされて DOM フォーカスをモーダルの入力欄から奪っていた。
   * ここでは「モーダルが可視のまま打鍵しても write_pty が飛ばない」ことを固定する。
   * 変異（modal === null の条件を外して無条件 focus に戻す）で赤くなることを実測した。
   */
  test('モーダル表示中に Cmd+2 でターミナルへ戻っても、打鍵が実シェルへ流れない', async ({
    page,
  }) => {
    await page.addInitScript(commonInitScript());
    await gotoTerminalView(page);

    await page.locator('[data-session-id="s1"]').click();
    await expect
      .poll(async () => {
        const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
        return calls.filter((c) => c.cmd === 'start_session').length;
      })
      .toBe(1);

    // Cmd+N はターミナル画面からも効く（契約 §11）。openModal は view を kanban へ
    // 切り替えるので、この時点で TerminalView（TerminalPane 含む）は一旦 unmount される
    await page.keyboard.press('Meta+n');
    await expect(page.locator('[role="dialog"]')).toBeVisible();

    // モーダルは view 分岐の外にあるため残ったまま、TerminalPane だけが再マウントされる
    await page.keyboard.press('Meta+2');
    await expect(page.locator('.kamux-terminal-view')).toBeVisible();
    await expect(page.locator('[role="dialog"]')).toBeVisible();

    await page.keyboard.type('x');

    const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
    expect(calls.filter((c) => c.cmd === 'write_pty' || c.cmd === 'write_pty_bytes')).toHaveLength(
      0,
    );
    // 宛先が正しくモーダルのタイトル入力へ届いていることも直接確認する
    await expect(page.locator('[role="dialog"] input[type="text"]').first()).toHaveValue('x');
  });

  /**
   * PR 10 fix round 1（Critical、契約側の追加裁定）。フォーカスは modal の遷移に
   * 追従する必要があり、「マウント時に 1 度評価する」だけでは、モーダルを開いてから
   * 閉じた後にターミナルが無フォーカスのまま残る別の穴が開く。ここでは
   * 「モーダルを閉じると、TerminalPane を再マウントすることなく打鍵が実シェルへ
   * 届くようになる」ことを固定する。変異（フォーカス effect の依存配列から modal を
   * 外す）で赤くなることを実測した。
   */
  test('モーダルを閉じると、再マウントなしでターミナルへの打鍵が再び効くようになる', async ({
    page,
  }) => {
    await page.addInitScript(commonInitScript());
    await gotoTerminalView(page);

    await page.locator('[data-session-id="s1"]').click();
    await expect
      .poll(async () => {
        const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
        return calls.filter((c) => c.cmd === 'start_session').length;
      })
      .toBe(1);

    await page.keyboard.press('Meta+n');
    await expect(page.locator('[role="dialog"]')).toBeVisible();
    await page.keyboard.press('Meta+2');
    await expect(page.locator('.kamux-terminal-view')).toBeVisible();

    // Escape はモーダル表示中に close_modal として effect される（契約 §11）。
    // view はここでは変わらないので TerminalPane は再マウントされない
    await page.keyboard.press('Escape');
    await expect(page.locator('[role="dialog"]')).toHaveCount(0);

    await page.keyboard.type('x');

    await expect
      .poll(async () => {
        const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
        return calls.filter(
          (c) =>
            c.cmd === 'write_pty' && (c.args as { surfaceId: string }).surfaceId === 's1:agent',
        ).length;
      })
      .toBe(1);
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
