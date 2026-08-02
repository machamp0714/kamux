import type { CreateSessionArgs } from '../../ipc/commands';
import { proposeBranchName } from '../../lib/branchName';
import type { CliKind, Session, SessionMode, SessionPatch } from '../../types/model';

export interface SessionFormValues {
  title: string;
  description: string;
  mode: SessionMode;
  /** 空文字 = 未指定。buildCreateSessionArgs が提案値または null へ解決する */
  branch: string;
  cliKind: CliKind;
  cliCommand: string;
}

export function initialSessionFormValues(defaultCli: CliKind): SessionFormValues {
  return {
    title: '',
    description: '',
    mode: 'worktree',
    branch: '',
    cliKind: defaultCli,
    cliCommand: '',
  };
}

export function sessionFormValuesFrom(session: Session): SessionFormValues {
  return {
    title: session.title,
    description: session.description,
    mode: session.mode,
    branch: session.branch ?? '',
    cliKind: session.cli_kind,
    cliCommand: session.cli_command ?? '',
  };
}

export function validateSessionForm(v: SessionFormValues): string[] {
  const errors: string[] = [];
  if (v.title.trim() === '') errors.push('タイトルは必須です');
  if (v.cliKind === 'custom' && v.cliCommand.trim() === '') {
    errors.push('custom CLI では起動コマンドが必須です');
  }
  return errors;
}

/** キーは camelCase。Tauri がコマンド引数名を snake_case へ自動変換する。 */
export function buildCreateSessionArgs(projectId: string, v: SessionFormValues): CreateSessionArgs {
  const title = v.title.trim();
  const typedBranch = v.branch.trim();
  return {
    projectId,
    title,
    description: v.description.trim(),
    mode: v.mode,
    // 契約 §13: in_place では git 操作を一切行わないので branch は必ず NULL
    branch:
      v.mode === 'in_place' ? null : typedBranch === '' ? proposeBranchName(title) : typedBranch,
    cliKind: v.cliKind,
    cliCommand: v.cliKind === 'custom' ? v.cliCommand.trim() || null : null,
  };
}

/**
 * 編集で変更できるのは title / description のみ（第1部 判断 10）。
 * SessionPatch（契約 §7）に mode / branch / cli_kind が無いのは意図的な制約。
 *
 * キーは **snake_case**。Tauri の camelCase 自動変換はコマンド引数名にしか効かず、
 * ネストした構造体のフィールドには効かない。camelCase で送ると型エラーにならず
 * 黙って無視される（第1部 判断 12）。
 */
export function buildSessionPatch(original: Session, v: SessionFormValues): SessionPatch {
  const patch: SessionPatch = {};
  const title = v.title.trim();
  const description = v.description.trim();
  if (title !== original.title) patch.title = title;
  if (description !== original.description) patch.description = description;
  return patch;
}
