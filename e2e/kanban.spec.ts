import { expect, test, type Locator, type Page } from '@playwright/test';
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

/**
 * ドラッグ操作の直後に別の要素をクリックする場合専用のヘルパー。
 *
 * @dnd-kit/core の PointerSensor#handleEnd は detach() を呼び、detach() は
 * 「ドロップ操作自身がその後に発生させる click イベント」を握りつぶすため、
 * document に capture: true な click リスナー（stopPropagation のみを呼ぶ）を仕込む。
 * このリスナーは即座には外れず setTimeout 50ms 後に外れる（documentListeners.removeAll。
 * node_modules/@dnd-kit/core/dist/core.esm.js の PointerSensor 実装を参照。ライブラリの
 * 意図的な仕様であり、kamux 側のバグではない）。
 *
 * capture:true かつ document に付いているため、この 50ms の窓に重なって別要素へ
 * クリックを送ると、その伝播も一緒に握りつぶされ、対象要素の React 側 onClick が
 * 一切発火しない（ネイティブの click イベント自体は正しい要素に届いているのに、
 * バブリングで document まで上がる前に止められる）。E2E を並列実行して負荷が高いと
 * ドラッグ完了からこのクリックまでの実時間が 50ms を下回りやすく、
 * 単発 click() だと稀に空振りする。
 *
 * 50ms という値そのものに依存しないよう、「効くまで打ち直す」形で待つ
 * （ライブラリの実装が変わっても壊れない）。ErrorToast 側の onClick が壊れていれば
 * 打ち直しても効かないので、最終的に toBeHidden() の本来のタイムアウトで正しく失敗する
 * （検出力は落ちない）。
 */
async function clickToastCloseUntilHidden(page: Page, toast: Locator) {
  const closeButton = page.locator('.error-toast__close');
  for (let attempt = 0; attempt < 10; attempt++) {
    // 直前の周回の click が実は効いていた（ただし toBeHidden の 100ms 待ちに
    // 間に合わなかっただけ）ケースを弾く。弾かないと、既に無い close ボタンへ
    // click() を打つことになり、actionTimeout 未設定のため test timeout(15s) まで
    // 待つ「別種の分かりにくい失敗」を生んでしまう。
    if (await toast.isHidden()) return;
    await closeButton.click();
    try {
      await expect(toast).toBeHidden({ timeout: 100 });
      return;
    } catch {
      // dnd-kit の click ガードに握りつぶされた可能性。打ち直す。
    }
  }
  // ここまでで外れているはずなので、ここは「本物の失敗」を正しいタイムアウトで検出する。
  await expect(toast).toBeHidden();
}

test.describe('カンバン操作（共通の 1 プロジェクト・2 セッション）', () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(
      tauriMockScript({
        commands: {
          // default_cli を 'claude' 以外にしてある。initialSessionFormValues の
          // フォールバック値も 'claude' なので、'claude' のままでは
          // 「プロジェクトから継承できている」ことに判別力が無い（第1部 §9.1 相当の穴）。
          list_projects: () => [
            { id: 'p1', name: 'kamux', repo_path: '/tmp/kamux', default_cli: 'codex' },
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
            // 同一列（backlog）内の入れ替え。フロント（resolveDragEnd / moveCardInOrder）と
            // 同じ規約 —— 移動対象 (s1) を除いた配列 L = [s2] に対する挿入 index として
            // args.toIndex を解釈し、並びをここで再現する。列間 DnD の分岐（args.toStatus）と
            // 同様に「モックが引数に依存する」構造にすることで、resolveDragEnd が誤った index
            // （例えば no-op の 0）を返したときにこの関数が返す並びも追従して壊れ、
            // 下の toHaveText アサーションが実際に落ちる（固定配列を読み返すだけの判別力ゼロを避ける）。
            const s1 = {
              id: 's1',
              project_id: 'p1',
              title: 'fix login',
              description: '',
              kanban_status: 'backlog',
              cli_kind: 'claude',
              mode: 'worktree',
              branch: 'fix-login',
              archived_at: null,
            };
            const s2 = {
              id: 's2',
              project_id: 'p1',
              title: 'add tests',
              description: '',
              kanban_status: 'backlog',
              cli_kind: 'claude',
              mode: 'worktree',
              branch: 'add-tests',
              archived_at: null,
            };
            const l = [s2];
            const insertAt = Math.max(0, Math.min(Number(args.toIndex), l.length));
            const order = [...l.slice(0, insertAt), s1, ...l.slice(insertAt)];
            return order.map((s, i) => ({ ...s, sort_order: (i + 1) * 1000 }));
          },
          // 契約 §4 の Session（18 フィールド）に一致させる。フォームの入力値を
          // そのまま返すことで、渡された args（cliKind の継承・branch の自動生成）を
          // 呼び出し履歴と戻り値の両方から検証できるようにする。
          create_session: (args) => ({
            id: 's3',
            project_id: args.projectId,
            title: args.title,
            description: args.description,
            kanban_status: 'backlog',
            sort_order: 3000,
            mode: args.mode,
            branch: args.branch,
            worktree_path: null,
            cli_kind: args.cliKind,
            cli_command: args.cliCommand,
            claude_session_id: null,
            last_runtime_state: 'idle',
            last_runtime_error: null,
            first_started_at: null,
            archived_at: null,
            created_at: Date.now(),
            updated_at: Date.now(),
          }),
        },
      }),
    );
    await page.goto('/');
    await expect(page.locator('[data-session-id="s1"]')).toBeVisible();

    // 第1部 §9「表示」: 4 列見出しの件数、カードの CLI アイコン・ブランチ名表示、
    // runtime バッジが 1 枚も出ないこと（フィクスチャは first_started_at === null なので
    // seedRuntimeStates が除外する。契約 §34.7 / §33.3 Q1）。
    // 数えるのはバッジ本体（.runtime-badge）である —— .kanban-card__badge は M2-1 Task 10 で
    // <RuntimeBadge> を置くための空スロットになり、バッジの有無を表さなくなった。
    await expect(page.locator('section[aria-label="Backlog"] .kanban-column__count')).toHaveText(
      '2',
    );
    await expect(
      page.locator('section[aria-label="In Progress"] .kanban-column__count'),
    ).toHaveText('0');
    await expect(page.locator('[data-column="backlog"] .kanban-card__cli')).toHaveCount(2);
    await expect(page.locator('[data-column="backlog"] .kanban-card__branch')).toHaveCount(2);
    await expect(page.locator('.runtime-badge')).toHaveCount(0);
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
    // over カード (s2) の移動前の位置（backlog = [s1, s2] における index 1）を
    // resolveDragEnd がそのまま返すこと（契約 §49.3.2 の to_index 規約）。
    expect(moveSessionCalls[0].args).toMatchObject({ id: 's1', toStatus: 'backlog', toIndex: 1 });

    // かつ、その toIndex を使って move_session モックが組み立てた並びが実際に DOM へ
    // 反映されていること（dnd-kit の over 解決の細部より、盤面に反映される最終結果の方が
    // 壊れにくい。上の toIndex アサーションと合わせて二重に判別力を持たせる）。
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

    // 第1部 §9「作成」: Escape で閉じたときカードは作られない
    // （create_session は未モックなので、呼ばれていれば unmocked command で reject し
    //  .error-toast が出るはずだが、それも起きていないことまで見る）。
    const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
    expect(calls.filter((c) => c.cmd === 'create_session')).toHaveLength(0);
    await expect(page.locator('.error-toast')).toHaveCount(0);
  });

  test('Cmd+N でセッションを作成すると Backlog 列の末尾にカードが現れる', async ({ page }) => {
    const dialog = page.getByRole('dialog', { name: '新規セッション' });
    await page.keyboard.press('Meta+n');
    await expect(dialog).toBeVisible();

    await dialog.getByPlaceholder('Fix login bug').fill('e2e new session');
    await dialog.getByRole('button', { name: '作成' }).click();
    await expect(dialog).toBeHidden();

    const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
    const createCalls = calls.filter((c) => c.cmd === 'create_session');
    expect(createCalls).toHaveLength(1);
    // cliKind はプロジェクトの default_cli ('codex') を継承する。branch は未編集のまま
    // 送信しているので null（契約 §62.3: 提案値は表示にのみ使い、DB へは焼かない。
    // 確定は prepare_worktree が worktree を実際に作る瞬間に行う）。
    expect(createCalls[0].args).toMatchObject({
      projectId: 'p1',
      title: 'e2e new session',
      mode: 'worktree',
      cliKind: 'codex',
      branch: null,
    });

    await expect(page.locator('[data-column="backlog"] .kanban-card__title')).toHaveText([
      'fix login',
      'add tests',
      'e2e new session',
    ]);
    await expect(page.locator('.error-toast')).toHaveCount(0);
  });

  test('ブランチ欄を手で編集すると、その入力値が create_session に渡る（契約 §51.3.2）', async ({
    page,
  }) => {
    const dialog = page.getByRole('dialog', { name: '新規セッション' });
    await page.keyboard.press('Meta+n');
    await expect(dialog).toBeVisible();

    await dialog.getByPlaceholder('Fix login bug').fill('e2e manual branch');
    await dialog.getByPlaceholder('作成時に自動生成されます').fill('feature/manual');
    await dialog.getByRole('button', { name: '作成' }).click();
    await expect(dialog).toBeHidden();

    const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
    const createCalls = calls.filter((c) => c.cmd === 'create_session');
    expect(createCalls).toHaveLength(1);
    expect(createCalls[0].args).toMatchObject({
      title: 'e2e manual branch',
      branch: 'feature/manual',
    });
  });

  test('kanban-card__actions はホバーまたはキーボードフォーカスで現れる', async ({ page }) => {
    const card = page.locator('[data-session-id="s1"]');
    const actions = card.locator('.kanban-card__actions');

    await expect(actions).toHaveCSS('opacity', '0');

    await card.hover();
    await expect(actions).toHaveCSS('opacity', '1');

    // ホバーを外してから確認しないと、直前の hover が残ったまま
    // :focus-within の効果を検証したことになってしまう。
    await page.mouse.move(0, 0);
    await expect(actions).toHaveCSS('opacity', '0');

    await card.getByRole('button', { name: '編集' }).focus();
    await expect(actions).toHaveCSS('opacity', '1');

    // カード自身がタブストップ（要件5 の Enter の受け口）である。そのフォーカスでも
    // .kanban-card:focus-within が一致してアクションが出ること —— ドラッグ用の
    // ラッパ側にタブストップがあると、ここが 0 のままになる。
    await page.mouse.move(0, 0);
    await card.getByRole('button', { name: '編集' }).blur();
    await expect(actions).toHaveCSS('opacity', '0');

    await card.focus();
    await expect(actions).toHaveCSS('opacity', '1');
  });

  test('Cmd+1 でカンバン画面が表示される', async ({ page }) => {
    const board = page.locator('.kanban-view');
    await expect(board).toBeVisible();

    // M1-2 の時点ではカンバン以外へ切り替える UI/キー操作が無いため、
    // ここで検証できるのは「Cmd+1 を押してもカンバンが表示されたままである」こと
    // （M1-3 以降が Cmd+2 を足したら、そちらから戻るケースを追加する）。
    await page.keyboard.press('Meta+1');
    await expect(board).toBeVisible();
    // M3-1 Task 7 が .app__placeholder（view === 'editor' の仮表示）を実物の
    // EditorView に置き換えたので、同じ意図（エディタ画面へ落ちていないこと）を
    // そのクラス名で見る。**両方の姿を見ること** —— EditorView は
    // focusedSessionId の有無で .kamux-editor-view / .editor-empty のどちらかを描く。
    // この経路ではカードを 1 枚もクリックしていないので focusedSessionId は null であり、
    // .kamux-editor-view だけを見ると App.tsx の view 分岐を外す変異でも緑のまま
    // （= 恒真の assert）になる。実測して赤くなることを確認済み（fix round 1）
    await expect(page.locator('.kamux-editor-view, .editor-empty')).toHaveCount(0);
  });
});

// 共通 describe の外に置く独立した spec。この spec だけ move_session が reject する
// モックが要り、共通 beforeEach の move_session（成功する版）と同じプロパティを
// 上書きする 2 本目の addInitScript を重ねる形は取らない —— Playwright は
// 「複数の addInitScript の評価順を保証しない」と明記しており、同じプロパティを
// 取り合う 2 スクリプトを重ねると理論上どちらが最終的に勝つか決まらない
// （たまたま今は後勝ちで動いていても、将来 flaky な失敗として顕在化しうる）。
// addInitScript は 1 回だけ、必要な形をあらかじめ全部詰めて呼ぶ。
test('move_session が失敗すると .error-toast が表示され、カードは元の列に戻る', async ({
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
  await page.goto('/');
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

  await clickToastCloseUntilHidden(page, toast);
});

test('起動時復元: activeProjectId の選択・sort_order 順の描画・不正 ID のフォールバックが UI に反映される', async ({
  page,
}) => {
  // localStorage が既に値を持つ場合（後段の reload）は上書きしない。
  // Playwright は複数 addInitScript の評価順を保証しないため、「後勝ち」に依存する
  // 設計は取らない。下の tauriMockScript の初期化スクリプトとは触るプロパティが
  // 完全に別（このスクリプトは localStorage、あちらは window.__TAURI_INTERNALS__）
  // なので、2 本の評価順がどちらであっても最終結果は変わらない。
  // このスクリプト自身も「値が無いときだけ書く」形にして、reload で再評価されても
  // （page.evaluate で書き換えた値を）壊さないようにしてある（下の fallback 検証で使う）。
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
  // 第1部 §9「表示」: in_place モードのカードにはブランチ名を出さない
  await expect(page.locator('.kanban-card__branch')).toHaveCount(0);

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

// PR 14 レビュー I-1（.superpowers/sdd/M2-1-runtime-state/pr14-verdict.md）:
// jsdom は外部 CSS を解決しないため、RuntimeBadge.test.tsx / KanbanCardError.test.tsx は
// インライン style の var(--state-*) 参照までしか固定できず、実ブラウザでの解決結果
// （var() の実色 / kanban.css の margin）を検証できない。ここは Vite dev server + 実 CSS +
// chromium で走る E2E にしかできない正のアサーションを持たせる。
//
// 契約 §33.5（ラベル）× §53.4（色トークン）の正典。唯一の非自明点は
// waiting_input → --state-waiting（トークン名に _input が付かない。RuntimeBadge.tsx 参照）。
const RUNTIME_STATE_CANON: Array<{
  sessionId: string;
  state: string;
  label: string;
  token: string;
}> = [
  { sessionId: 's-running', state: 'running', label: '実行中', token: '--state-running' },
  { sessionId: 's-waiting', state: 'waiting_input', label: '入力待ち', token: '--state-waiting' },
  { sessionId: 's-idle', state: 'idle', label: 'アイドル', token: '--state-idle' },
  { sessionId: 's-exited', state: 'exited', label: '終了', token: '--state-exited' },
  { sessionId: 's-interrupted', state: 'interrupted', label: '中断', token: '--state-interrupted' },
  { sessionId: 's-error', state: 'error', label: 'エラー', token: '--state-error' },
];

test('runtime バッジが実ブラウザで描かれ、var(--state-*) が実色に解決される（契約 §33.5 / §53.4）', async ({
  page,
}) => {
  await page.addInitScript(
    tauriMockScript({
      commands: {
        list_projects: () => [
          { id: 'p1', name: 'kamux', repo_path: '/tmp/kamux', default_cli: 'claude' },
        ],
        list_sessions: () =>
          ['running', 'waiting_input', 'idle', 'exited', 'interrupted', 'error'].map(
            (state, i) => ({
              id: `s-${state === 'waiting_input' ? 'waiting' : state}`,
              project_id: 'p1',
              title: state,
              description: '',
              kanban_status: 'backlog',
              sort_order: (i + 1) * 1000,
              cli_kind: 'claude',
              mode: 'worktree',
              branch: `branch-${state}`,
              archived_at: null,
              last_runtime_state: state,
              // ❌ の枠は別 spec（.kanban-card__error）の担当。ここでは badge だけを見たいので
              // 空にしておく（KanbanCardError は message が無ければ描かない）。
              last_runtime_error: null,
              first_started_at: 1000,
            }),
          ),
      },
    }),
  );
  await page.goto('/');
  await expect(page.locator('[data-session-id="s-running"]')).toBeVisible();

  // 負の主張（.runtime-badge が 0 件）は e2e/kanban.spec.ts の共通 beforeEach が別途固定している
  // （未起動のカードにバッジを出さない、契約 §34.6）。ここはその逆 —— 起動済みの 6 セッションに
  // 対して実際に 6 枚描かれることを見る正の主張。
  await expect(page.locator('.runtime-badge')).toHaveCount(RUNTIME_STATE_CANON.length);

  for (const { sessionId, state, label, token } of RUNTIME_STATE_CANON) {
    const badge = page.locator(`[data-session-id="${sessionId}"] .runtime-badge`);
    await expect(badge).toHaveAttribute('data-runtime-state', state);
    await expect(badge.locator('.runtime-badge__label')).toHaveText(label);

    const dotColor = await badge
      .locator('.runtime-badge__dot')
      .evaluate((el) => getComputedStyle(el).backgroundColor);

    // var(--state-*) が未解決だと、color は継承チェーンへフォールバックし
    // token 固有の色とはズレた（が一見もっともらしい）rgb() を返す —— 「空文字/透明かどうか」
    // だけでは検出できない。同じトークン名を直接参照した probe 要素の解決結果と
    // 一致することだけを見る（特定の色値をハードコードしないので、テーマが変わっても通る）。
    const expectedColor = await page.evaluate((t) => {
      const probe = document.createElement('div');
      probe.style.color = `var(${t})`;
      document.body.appendChild(probe);
      const value = getComputedStyle(probe).color;
      probe.remove();
      return value;
    }, token);

    expect(dotColor).toMatch(/^rgb\(\d+, \d+, \d+\)$/);
    expect(dotColor).toBe(expectedColor);
  }
});

test('.kanban-card__error が実ブラウザで描かれ、生 stderr が原文で出て margin が潰れている（契約 §42.4）', async ({
  page,
}) => {
  // tauriMockScript の commands はブラウザ側で toString() 経由で再評価される
  // （外側の変数を参照するとブラウザ側では undefined になる。tauriMock.ts のコメント参照）。
  // そのため文字列リテラルは mock 側とアサーション側の両方に重複して書く。
  const rawStderr = 'error: failed to spawn\n  caused by: ENOENT';

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
            last_runtime_state: 'error',
            last_runtime_error: 'error: failed to spawn\n  caused by: ENOENT',
            // 契約 §40.5: 起動に一度も成功していないセッションでも 'error' は必ず seed される
            // （first_started_at === null の除外例外）。
            first_started_at: null,
          },
          // 対照カード。:has() が「選択している」ことの向きを測るために要る（下の assert 参照）。
          // last_runtime_error を null にしておくこと —— 非 null だと KanbanCardError が
          // 2 枚目の .kanban-card__error を描き、上の toBeVisible() が strict mode violation で
          // 落ちて、このテストの本題（error 側）ごと意味が変わる。
          {
            id: 's2',
            project_id: 'p1',
            title: 'no error',
            description: '',
            kanban_status: 'backlog',
            sort_order: 2000,
            cli_kind: 'claude',
            mode: 'worktree',
            branch: 'no-error',
            archived_at: null,
            last_runtime_state: 'idle',
            last_runtime_error: null,
            first_started_at: null,
          },
        ],
      },
    }),
  );
  await page.goto('/');
  await expect(page.locator('[data-session-id="s1"]')).toBeVisible();

  const errorBox = page.locator('.kanban-card__error');
  await expect(errorBox).toBeVisible();
  // toHaveText はデフォルトで空白を正規化するため、改行を含む原文の一致確認には使わない
  // （契約 §42.4「生 stderr を原文で」）。
  expect(await errorBox.textContent()).toBe(rawStderr);

  // Task 10 fix1（kanban.css）が足した `margin: 0` の固定。<pre> の UA 既定
  // margin: 1em 0 が復活していないこと。jsdom は外部 CSS を解決しないため
  // KanbanCardError.test.tsx では原理的に検出できない（このテストの本題）。
  expect(await errorBox.evaluate((el) => getComputedStyle(el).margin)).toBe('0px');

  // 契約 §76.5（M3-4 Task 9）: ❌ のカードは枠が赤くなる。実装は
  // `.kanban-card:has(.kanban-card__error)` であり、修飾子クラスを持たないので
  // DOM からは観測できない。:has() の解決も computed style でしか見えないため、
  // jsdom（KanbanCard.test.tsx）では原理的に検出できずここが唯一の観測点になる。
  const borderColor = await page
    .locator('[data-session-id="s1"].kanban-card')
    .evaluate((el) => getComputedStyle(el).borderTopColor);

  // 上のバッジ spec と同じ probe。var(--state-error) が未解決だと継承チェーンへ
  // フォールバックして「一見もっともらしい rgb()」を返すため、色値をハードコードせず
  // 同じトークンを直接参照した probe の解決結果と突き合わせる。
  const expectedBorderColor = await page.evaluate(() => {
    const probe = document.createElement('div');
    probe.style.color = 'var(--state-error)';
    document.body.appendChild(probe);
    const value = getComputedStyle(probe).color;
    probe.remove();
    return value;
  });

  expect(borderColor).toMatch(/^rgb\(\d+, \d+, \d+\)$/);
  expect(borderColor).toBe(expectedBorderColor);

  // ここまでの assert には向きが無い ——「❌ のカードだけが赤枠」と「全カードが赤枠」
  // （= .kanban-card の素の border を赤にしただけ）を区別しない。:has() が実際に
  // 選択していることは、エラーでない対照カードの枠が既定色のままであることでしか測れない。
  // 対照側も probe 解決値との toBe で見るので、error 側とまったく同じ強さになる
  // （「異なる」assert だと --border 以外の何色でも通ってしまう）。
  // ホバーしないこと —— .kanban-card:hover は border-color: var(--border-strong) を当てるので、
  // hover したまま測ると変異と無関係に落ちる。
  await expect(page.locator('[data-session-id="s2"]')).toBeVisible();
  const controlBorderColor = await page
    .locator('[data-session-id="s2"].kanban-card')
    .evaluate((el) => getComputedStyle(el).borderTopColor);

  const expectedControlBorderColor = await page.evaluate(() => {
    const probe = document.createElement('div');
    probe.style.color = 'var(--border)';
    document.body.appendChild(probe);
    const value = getComputedStyle(probe).color;
    probe.remove();
    return value;
  });

  // 2 つの probe が同じ値に解決されると、下の toBe は「全カードが赤枠」でも通ってしまう。
  // 母数が生きていることの確認（恒真ではない —— --border と --state-error が
  // 同じ色に変えられたらここが落ちる）。
  expect(expectedControlBorderColor).not.toBe(expectedBorderColor);
  expect(controlBorderColor).toBe(expectedControlBorderColor);
});
