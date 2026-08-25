import { expect, test, type Page } from '@playwright/test';
import { tauriMockScript } from './support/tauriMock';

/**
 * M2-4（セッション再開）の DOM 観測を固定する spec。
 *
 * 受け皿になった経緯: `docs/superpowers/plans/2026-08-01-kamux/M2-4-resume.md` §6.0(a) の
 * 仕分け表（7 行）。「観測が DOM の 1 要素で完結し、IPC の向こう側（実 Rust / 実 PTY /
 * 実 SQLite / 実 git / アプリ再起動）を 1 度も通らない」項目だけをここへ移した。
 * 先例は `e2e/heuristics.spec.ts`（同じ理由で手動スモークから追い出された観測の受け皿）。
 *
 * `KanbanCardResume.test.tsx`（vitest）と `resumeAffordance.test.ts` が既にラベル 4 分岐の
 * 総当たりを持つ。ここに置くのは **実ストア + 実 `listen()` 配線 + 実カード（dnd-kit の
 * context の中）を跨ぐ経路**だけである（契約 §26）。
 *
 * `⚠ ${note}` の前置を描くコンポーネントは M3-4 で撤去された（契約 §126.4 の 2 / §150）
 * ため、ここでは移さない。カード側（`KanbanCardResume.tsx:60`）は `title={note}` のみで
 * ⚠ を付けないので、`title` 属性の値だけを見る。
 *
 * fixture の前提（落とすと全 assert が無関係な理由で赤くなる）:
 * - `runtimeStates[id]` が `interrupted` または `exited` であること
 *   （`KanbanCardResume.tsx` のゲートが `null` を返す条件。§6.0(a) の M-GATE）
 * - `first_started_at` が非 null であること（`seedRuntimeStates` は null を seed から除外する。
 *   `e2e/heuristics.spec.ts` の doc コメントと同じ罠）
 */

function mockScript(): string {
  return tauriMockScript({
    commands: {
      list_projects: () => [
        { id: 'p1', name: 'kamux', repo_path: '/tmp/kamux', default_cli: 'claude' },
      ],
      // ブラウザ側で toString() 経由で再評価されるため、外側の変数を参照しないこと
      list_sessions: () => [
        // 行 1・2: claude_session_id 済み + interrupted → 「会話を再開」+ resume_session
        {
          id: 's-resume',
          project_id: 'p1',
          title: 'resume target',
          description: '',
          kanban_status: 'backlog',
          sort_order: 1000,
          mode: 'worktree',
          branch: 'resume-branch',
          worktree_path: '/tmp/kamux-wt/resume',
          cli_kind: 'claude',
          cli_command: null,
          claude_session_id: 'claude-sess-resume',
          last_runtime_state: 'interrupted',
          last_runtime_error: null,
          first_started_at: 1000,
          heuristics_enabled: false,
          silence_timeout_secs: 30,
          archived_at: null,
        },
        // 行 3: claude_session_id null + worktree → 「会話を再開」+ --continue の注記
        {
          id: 's-continue',
          project_id: 'p1',
          title: 'continue target',
          description: '',
          kanban_status: 'backlog',
          sort_order: 2000,
          mode: 'worktree',
          branch: 'continue-branch',
          worktree_path: '/tmp/kamux-wt/continue',
          cli_kind: 'claude',
          cli_command: null,
          claude_session_id: null,
          last_runtime_state: 'interrupted',
          last_runtime_error: null,
          first_started_at: 1000,
          heuristics_enabled: false,
          silence_timeout_secs: 30,
          archived_at: null,
        },
        // 行 4: claude_session_id null + in_place → 「新しい会話で開始」+ 曖昧回避の注記
        {
          id: 's-inplace',
          project_id: 'p1',
          title: 'in_place target',
          description: '',
          kanban_status: 'backlog',
          sort_order: 3000,
          mode: 'in_place',
          branch: null,
          worktree_path: null,
          cli_kind: 'claude',
          cli_command: null,
          claude_session_id: null,
          last_runtime_state: 'interrupted',
          last_runtime_error: null,
          first_started_at: 1000,
          heuristics_enabled: false,
          silence_timeout_secs: 30,
          archived_at: null,
        },
        // 行 5: resume_failed を合成 emit してから使う。初期状態は行 1 と同じ（会話を再開）
        {
          id: 's-failed',
          project_id: 'p1',
          title: 'failed target',
          description: '',
          kanban_status: 'backlog',
          sort_order: 4000,
          mode: 'worktree',
          branch: 'failed-branch',
          worktree_path: '/tmp/kamux-wt/failed',
          cli_kind: 'claude',
          cli_command: null,
          claude_session_id: 'claude-sess-failed',
          last_runtime_state: 'interrupted',
          last_runtime_error: null,
          first_started_at: 1000,
          heuristics_enabled: false,
          silence_timeout_secs: 30,
          archived_at: null,
        },
        // 行 6: cli_kind が claude 以外 → 「プロセスを再起動」+ 会話は復元されない注記
        {
          id: 's-shell',
          project_id: 'p1',
          title: 'shell target',
          description: '',
          kanban_status: 'backlog',
          sort_order: 5000,
          mode: 'in_place',
          branch: null,
          worktree_path: null,
          cli_kind: 'shell',
          cli_command: 'npm run dev',
          claude_session_id: null,
          last_runtime_state: 'interrupted',
          last_runtime_error: null,
          first_started_at: 1000,
          heuristics_enabled: true,
          silence_timeout_secs: 30,
          archived_at: null,
        },
        // 行 7: running を合成 emit してボタンが消えることを見る（UI 側の門。M-GATE の陽性対照）
        {
          id: 's-gate',
          project_id: 'p1',
          title: 'gate target',
          description: '',
          kanban_status: 'backlog',
          sort_order: 6000,
          mode: 'worktree',
          branch: 'gate-branch',
          worktree_path: '/tmp/kamux-wt/gate',
          cli_kind: 'claude',
          cli_command: null,
          claude_session_id: 'claude-sess-gate',
          last_runtime_state: 'interrupted',
          last_runtime_error: null,
          first_started_at: 1000,
          heuristics_enabled: false,
          silence_timeout_secs: 30,
          archived_at: null,
        },
      ],
      // 呼び出し履歴（引数）だけを見るので戻り値の内容自体に意味は無いが、
      // Session 形の値を返して後続の描画がエラーにならないようにする。
      resume_session: (args) => ({
        id: args.id,
        project_id: 'p1',
        title: 'resumed',
        description: '',
        kanban_status: 'backlog',
        sort_order: 1000,
        mode: 'worktree',
        branch: null,
        worktree_path: null,
        cli_kind: 'claude',
        cli_command: null,
        claude_session_id: 'resumed-session-id',
        last_runtime_state: 'running',
        last_runtime_error: null,
        first_started_at: 2000,
        heuristics_enabled: false,
        silence_timeout_secs: 30,
        archived_at: null,
      }),
      update_session: (args) => ({
        id: args.id,
        project_id: 'p1',
        title: 'updated',
        description: '',
        kanban_status: 'backlog',
        sort_order: 1000,
        mode: 'worktree',
        branch: null,
        worktree_path: null,
        cli_kind: 'claude',
        cli_command: null,
        claude_session_id: null,
        last_runtime_state: 'interrupted',
        last_runtime_error: null,
        first_started_at: 1000,
        heuristics_enabled: false,
        silence_timeout_secs: 30,
        archived_at: null,
      }),
    },
  });
}

/** カードの `.kanban-card__actions` 内で最初に描かれるボタン（KanbanCard.tsx: KanbanCardResume
 *  が編集/アーカイブより先にマウントされる）。まだホバーしていないと opacity: 0 +
 *  pointer-events: none なので、呼ぶ前にカード自身を hover しておくこと。 */
function resumeButtonOf(page: Page, sessionId: string) {
  return page.locator(`[data-session-id="${sessionId}"] .kanban-card__actions button`).first();
}

/** `session://state/{session_id}` を合成 emit して outcome が安定するまで打ち直す。
 *  StrictMode の mount→unmount→mount 直後は listen() がまだ pending の窓があり、
 *  そこで 1 度だけ emit すると無音で落ちる（`e2e/heuristics.spec.ts` と同じ理由）。 */
async function emitStateUntil(
  page: Page,
  args: { sessionId: string; runtimeState: string; reason: string },
  outcome: () => Promise<string | number>,
  expected: string | number,
): Promise<void> {
  await expect
    .poll(async () => {
      await page.evaluate(({ sessionId, runtimeState, reason }) => {
        window.__TAURI_INTERNALS__.__kamuxEmit(`session://state/${sessionId}`, {
          session_id: sessionId,
          runtime_state: runtimeState,
          reason,
        });
      }, args);
      return outcome();
    })
    .toBe(expected);
}

test('claude_session_id 済み + interrupted のカードは「会話を再開」を描き、クリックで resume_session が { id } で invoke される（§6.0(a) の 1・2）', async ({
  page,
}) => {
  await page.addInitScript(mockScript());
  await page.goto('/');
  const card = page.locator('[data-session-id="s-resume"]');
  await expect(card).toBeVisible();
  await card.hover();

  const button = resumeButtonOf(page, 's-resume');
  await expect(button).toHaveText('会話を再開');

  await button.click();

  await expect
    .poll(async () => {
      const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
      return calls.some((c) => c.cmd === 'resume_session');
    })
    .toBe(true);

  const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
  const resumeCall = calls.find((c) => c.cmd === 'resume_session');
  expect(resumeCall?.args).toMatchObject({ id: 's-resume' });
});

test('claude_session_id が null + worktree のカードは「会話を再開」と --continue の注記を出す（§6.0(a) の 3）', async ({
  page,
}) => {
  await page.addInitScript(mockScript());
  await page.goto('/');
  const card = page.locator('[data-session-id="s-continue"]');
  await expect(card).toBeVisible();
  await card.hover();

  const button = resumeButtonOf(page, 's-continue');
  await expect(button).toHaveText('会話を再開');
  await expect(button).toHaveAttribute('title', 'この作業ツリーの最新の会話に接続します');
});

test('claude_session_id が null + in_place のカードは「新しい会話で開始」と曖昧回避の注記を出す（§6.0(a) の 4。M-LABEL の対象）', async ({
  page,
}) => {
  await page.addInitScript(mockScript());
  await page.goto('/');
  const card = page.locator('[data-session-id="s-inplace"]');
  await expect(card).toBeVisible();
  await card.hover();

  const button = resumeButtonOf(page, 's-inplace');
  await expect(button).toHaveText('新しい会話で開始');
  await expect(button).toHaveAttribute(
    'title',
    'この作業ツリーの会話を特定できないため、新しい会話として開始します',
  );
});

test('resume_failed を受けたカードはボタンが「新しい会話として開始」に切り替わり、クリックで claude_session_id クリア → resume_session の順に invoke される（§6.0(a) の 5。M-FAILED / M-PATCH の対象）', async ({
  page,
}) => {
  await page.addInitScript(mockScript());
  await page.goto('/');
  const card = page.locator('[data-session-id="s-failed"]');
  await expect(card).toBeVisible();
  await card.hover();

  const button = resumeButtonOf(page, 's-failed');
  await expect(button).toHaveText('会話を再開');

  await emitStateUntil(
    page,
    { sessionId: 's-failed', runtimeState: 'exited', reason: 'resume_failed' },
    () => button.textContent().then((t) => t ?? ''),
    '新しい会話として開始',
  );

  await button.click();

  await expect
    .poll(async () => {
      const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
      return calls.some(
        (c) => c.cmd === 'resume_session' && (c.args as { id: string }).id === 's-failed',
      );
    })
    .toBe(true);

  const calls = await page.evaluate(() => window.__TAURI_INTERNALS__.__kamuxCalls);
  const updateIdx = calls.findIndex(
    (c) => c.cmd === 'update_session' && (c.args as { id: string }).id === 's-failed',
  );
  const resumeIdx = calls.findIndex(
    (c) => c.cmd === 'resume_session' && (c.args as { id: string }).id === 's-failed',
  );
  expect(updateIdx).toBeGreaterThanOrEqual(0);
  expect(resumeIdx).toBeGreaterThan(updateIdx);
  expect(calls[updateIdx].args).toMatchObject({
    id: 's-failed',
    patch: { claude_session_id: null },
  });
});

test('cli_kind が claude 以外の interrupted カードは「プロセスを再起動」と会話が復元されない注記を出す（§6.0(a) の 6）', async ({
  page,
}) => {
  await page.addInitScript(mockScript());
  await page.goto('/');
  const card = page.locator('[data-session-id="s-shell"]');
  await expect(card).toBeVisible();
  await card.hover();

  const button = resumeButtonOf(page, 's-shell');
  await expect(button).toHaveText('プロセスを再起動');
  await expect(button).toHaveAttribute('title', '会話は復元されません');
});

test('running を受け取ると再開ボタンが DOM から消える —— UI 側の門（§6.0(a) の 7。M-GATE の陽性対照）', async ({
  page,
}) => {
  await page.addInitScript(mockScript());
  await page.goto('/');
  const card = page.locator('[data-session-id="s-gate"]');
  await expect(card).toBeVisible();
  await card.hover();

  const button = resumeButtonOf(page, 's-gate');
  await expect(button).toHaveText('会話を再開');

  await emitStateUntil(
    page,
    { sessionId: 's-gate', runtimeState: 'running', reason: 'spawned' },
    () => card.locator('.kanban-card__actions button').count(),
    2, // 編集・アーカイブのみ残る（再開ボタンは消える）
  );
});
