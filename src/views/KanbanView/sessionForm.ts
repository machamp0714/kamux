import type { CreateSessionArgs } from '../../ipc/commands';
import type { CliKind, Session, SessionMode, SessionPatch } from '../../types/model';

export interface SessionFormValues {
  title: string;
  description: string;
  mode: SessionMode;
  /** 入力欄に表示されている文字列。branchTouched が false のときは提案値の表示にのみ使い、送信しない。 */
  branch: string;
  /**
   * true ならユーザーがブランチ欄を編集済み。buildCreateSessionArgs はこのときだけ
   * branch を CreateSessionArgs へ載せる（契約 §62.3。proposeBranchName の出力を DB へ焼かない）。
   */
  branchTouched: boolean;
  cliKind: CliKind;
  cliCommand: string;
}

export function initialSessionFormValues(defaultCli: CliKind): SessionFormValues {
  return {
    title: '',
    description: '',
    mode: 'worktree',
    branch: '',
    branchTouched: false,
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
    // 既存セッションの branch は既に確定した値なので編集済み扱いにする
    // （編集ダイアログでは SessionPatch に branch が無いため実際には送信されない。契約 §7）。
    branchTouched: true,
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
    // 契約 §13: in_place では git 操作を一切行わないので branch は必ず NULL。
    // worktree モードでも、ユーザーが欄を編集していなければ branch は送らない
    // （契約 §62.3: proposeBranchName の出力を CreateSessionArgs.branch へ到達させない。
    //   確定は prepare_worktree が worktree を実際に作る瞬間に行う）。
    branch:
      v.mode === 'in_place' ? null : v.branchTouched && typedBranch !== '' ? typedBranch : null,
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
