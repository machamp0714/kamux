import { describe, expect, it } from 'vitest';
import type { Session } from '../../types/model';
import {
  buildCreateSessionArgs,
  buildSessionPatch,
  initialSessionFormValues,
  sessionFormValuesFrom,
  validateSessionForm,
  type SessionFormValues,
} from './sessionForm';

function values(overrides: Partial<SessionFormValues> = {}): SessionFormValues {
  return {
    title: 'Fix Login Bug',
    description: 'ログインが落ちる',
    mode: 'worktree',
    branch: '',
    cliKind: 'claude',
    cliCommand: '',
    ...overrides,
  };
}

function makeSession(overrides: Partial<Session> & { id: string }): Session {
  return {
    project_id: 'p1',
    title: 'Fix Login Bug',
    description: 'ログインが落ちる',
    kanban_status: 'backlog',
    sort_order: 1,
    mode: 'worktree',
    branch: 'session/fix-login-bug',
    worktree_path: null,
    cli_kind: 'claude',
    cli_command: null,
    claude_session_id: null,
    last_runtime_state: 'idle',
    last_runtime_error: null,
    first_started_at: 1,
    archived_at: null,
    created_at: 0,
    updated_at: 0,
    ...overrides,
  };
}

describe('initialSessionFormValues', () => {
  it('プロジェクトの default_cli を初期値にし、モードは worktree で始まる', () => {
    expect(initialSessionFormValues('codex')).toEqual({
      title: '',
      description: '',
      mode: 'worktree',
      branch: '',
      cliKind: 'codex',
      cliCommand: '',
    });
  });
});

describe('sessionFormValuesFrom', () => {
  it('既存セッションをフォーム値へ写す（null は空文字にする）', () => {
    expect(
      sessionFormValuesFrom(makeSession({ id: 's1', branch: null, cli_command: null })),
    ).toEqual({
      title: 'Fix Login Bug',
      description: 'ログインが落ちる',
      mode: 'worktree',
      branch: '',
      cliKind: 'claude',
      cliCommand: '',
    });
  });
});

describe('validateSessionForm', () => {
  it('妥当な入力では空配列を返す', () => {
    expect(validateSessionForm(values())).toEqual([]);
  });

  it('タイトルが空白のみならエラーを返す', () => {
    expect(validateSessionForm(values({ title: '   ' }))).toEqual(['タイトルは必須です']);
  });

  it('custom CLI で起動コマンドが空ならエラーを返す', () => {
    expect(validateSessionForm(values({ cliKind: 'custom', cliCommand: ' ' }))).toEqual([
      'custom CLI では起動コマンドが必須です',
    ]);
  });

  it('custom CLI でも起動コマンドがあれば妥当', () => {
    expect(validateSessionForm(values({ cliKind: 'custom', cliCommand: 'aider' }))).toEqual([]);
  });
});

describe('buildCreateSessionArgs', () => {
  it('キーは camelCase（Tauri がコマンド引数名を snake_case へ変換する）', () => {
    expect(Object.keys(buildCreateSessionArgs('p1', values())).sort()).toEqual([
      'branch',
      'cliCommand',
      'cliKind',
      'description',
      'mode',
      'projectId',
      'title',
    ]);
  });

  it('タイトルと説明を trim して渡す', () => {
    const args = buildCreateSessionArgs(
      'p1',
      values({ title: '  Fix Login Bug  ', description: ' x ' }),
    );
    expect(args.projectId).toBe('p1');
    expect(args.title).toBe('Fix Login Bug');
    expect(args.description).toBe('x');
  });

  it('ブランチ欄が空ならタイトルから提案した名前を使う', () => {
    expect(buildCreateSessionArgs('p1', values()).branch).toBe('session/fix-login-bug');
  });

  it('ブランチ欄に入力があればそれを優先する', () => {
    expect(buildCreateSessionArgs('p1', values({ branch: ' feature/manual ' })).branch).toBe(
      'feature/manual',
    );
  });

  it('slug が空になるタイトルでは branch を null にする', () => {
    expect(buildCreateSessionArgs('p1', values({ title: '日本語タイトル' })).branch).toBeNull();
  });

  it('mode が in_place ならブランチ欄に文字があっても null を送る（契約 §13）', () => {
    const args = buildCreateSessionArgs(
      'p1',
      values({ mode: 'in_place', branch: 'feature/manual' }),
    );
    expect(args.mode).toBe('in_place');
    expect(args.branch).toBeNull();
  });

  it('cli_kind が custom 以外なら cliCommand を null にする', () => {
    expect(
      buildCreateSessionArgs('p1', values({ cliKind: 'shell', cliCommand: 'zsh' })).cliCommand,
    ).toBeNull();
  });

  it('cli_kind が custom なら cliCommand を trim して渡す', () => {
    expect(
      buildCreateSessionArgs('p1', values({ cliKind: 'custom', cliCommand: ' aider ' })).cliCommand,
    ).toBe('aider');
  });
});

describe('buildSessionPatch', () => {
  it('キーは snake_case（Tauri の自動変換はネストした構造体に効かない）', () => {
    const patch = buildSessionPatch(
      makeSession({ id: 's1' }),
      values({ title: 'A', description: 'B' }),
    );
    expect(Object.keys(patch).sort()).toEqual(['description', 'title']);
    expect(patch).not.toHaveProperty('kanbanStatus');
    expect(patch).not.toHaveProperty('sortOrder');
  });

  it('変更のあった項目だけを含める', () => {
    expect(buildSessionPatch(makeSession({ id: 's1' }), values({ title: '別のタイトル' }))).toEqual(
      { title: '別のタイトル' },
    );
  });

  it('変更がなければ空のパッチを返す', () => {
    expect(buildSessionPatch(makeSession({ id: 's1' }), values())).toEqual({});
  });

  it('SessionPatch に無いフィールド（mode / branch / cli_kind）は含めない（第1部 判断 10）', () => {
    const patch = buildSessionPatch(
      makeSession({ id: 's1' }),
      values({ mode: 'in_place', branch: 'other', cliKind: 'shell' }),
    );
    expect(patch).toEqual({});
  });

  it('archived_at は扱わない（アーカイブは archiveSession の責務）', () => {
    const patch = buildSessionPatch(
      makeSession({ id: 's1', archived_at: 1754006400000 }),
      values(),
    );
    expect(patch).not.toHaveProperty('archived_at');
  });
});

describe('SessionPatch の archived_at 3 値（契約 §7.3）', () => {
  // Rust 側は Option<Option<i64>> + 専用デシリアライザで
  // キー不在 = 変更しない / null = アーカイブ解除 / 数値 = アーカイブ を区別する。
  // TS 側で null が undefined に潰れると解除が無言の no-op になるので、
  // patch を組み立てる関数が null を落とさないことをここで踏み固める（解除 UI は M3-4）。
  it('null を持つ patch を JSON 化しても null が残る', () => {
    expect(JSON.parse(JSON.stringify({ archived_at: null }))).toEqual({ archived_at: null });
  });

  it('undefined を持つ patch は JSON 化でキーごと落ちる（= 変更しない）', () => {
    expect(JSON.parse(JSON.stringify({ archived_at: undefined }))).toEqual({});
  });
});
