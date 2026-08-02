import { useEffect, useState, type FormEvent } from 'react';
import { useAppStore } from '../../store';
import { toAppError, type ModalState } from '../../store/uiSlice';
import { proposeBranchName } from '../../lib/branchName';
import type { CliKind, SessionMode } from '../../types/model';
import { resolveDialogMode } from './dialogMode';
import {
  buildCreateSessionArgs,
  buildSessionPatch,
  initialSessionFormValues,
  sessionFormValuesFrom,
  validateSessionForm,
  type SessionFormValues,
} from './sessionForm';

const CLI_KINDS: CliKind[] = ['claude', 'codex', 'shell', 'custom'];
const MODES: SessionMode[] = ['worktree', 'in_place'];

/**
 * modal の内容が変わるたびに key で作り直すことで、フォームの初期化を
 * useEffect ではなく useState の初期化子ひとつで済ませる。
 */
export function SessionFormModal() {
  const modal = useAppStore((s) => s.modal);
  if (modal === null) return null;
  const key = modal.kind === 'edit_session' ? modal.sessionId : 'create';
  return <SessionFormDialog key={key} modal={modal} />;
}

function SessionFormDialog({ modal }: { modal: ModalState }) {
  const closeModal = useAppStore((s) => s.closeModal);
  const addSession = useAppStore((s) => s.addSession);
  const editSession = useAppStore((s) => s.editSession);
  const setError = useAppStore((s) => s.setError);
  const project = useAppStore((s) => s.projects.find((p) => p.id === s.activeProjectId) ?? null);
  const editingSession = useAppStore((s) =>
    modal.kind === 'edit_session' ? (s.sessions[modal.sessionId] ?? null) : null,
  );
  const dialogMode = resolveDialogMode(modal, editingSession);

  const [values, setValues] = useState<SessionFormValues>(() =>
    dialogMode.kind === 'edit'
      ? sessionFormValuesFrom(dialogMode.session)
      : initialSessionFormValues(project?.default_cli ?? 'claude'),
  );
  // 編集時は既存のブランチ名を守るため、最初から追従を止めておく
  const [branchTouched, setBranchTouched] = useState(dialogMode.kind === 'edit');
  const [busy, setBusy] = useState(false);

  // 編集モードで開いている間に対象セッションがストアから消えた場合（アーカイブ等）、
  // 作成モードへフォールバックせず閉じる（dialogMode.ts 参照）。
  useEffect(() => {
    if (dialogMode.kind === 'lost') closeModal();
  }, [dialogMode.kind, closeModal]);

  if (dialogMode.kind === 'lost') return null;

  const errors = validateSessionForm(values);
  const isEdit = dialogMode.kind === 'edit';
  // 作成モードでは activeProjectId 先のプロジェクトが要る。無ければ黙って
  // 閉じるのではなく送信自体を止める（バリデーションエラーの一種として扱う）。
  const canSubmit = errors.length === 0 && !busy && (isEdit || project !== null);

  const onTitleChange = (title: string) => {
    setValues((v) => ({
      ...v,
      title,
      branch: branchTouched ? v.branch : (proposeBranchName(title) ?? ''),
    }));
  };

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!canSubmit) return;
    setBusy(true);
    try {
      if (dialogMode.kind === 'edit') {
        const patch = buildSessionPatch(dialogMode.session, values);
        if (Object.keys(patch).length > 0) await editSession(dialogMode.session.id, patch);
      } else if (project !== null) {
        await addSession(buildCreateSessionArgs(project.id, values));
      }
      closeModal();
    } catch (err: unknown) {
      setError(toAppError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="session-form-modal__backdrop" onMouseDown={closeModal}>
      <div
        className="session-form-modal"
        role="dialog"
        aria-modal="true"
        aria-label={isEdit ? 'セッションを編集' : '新規セッション'}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <form
          onSubmit={(e) => {
            void onSubmit(e);
          }}
        >
          <header className="session-form-modal__header">
            <h2 className="session-form-modal__title">
              {isEdit ? 'セッションを編集' : '新規セッション'}
            </h2>
          </header>

          <div className="session-form-modal__body">
            <label className="session-form-modal__field">
              <span>タイトル</span>
              <input
                autoFocus
                type="text"
                value={values.title}
                onChange={(e) => onTitleChange(e.target.value)}
                placeholder="Fix login bug"
              />
            </label>

            <label className="session-form-modal__field">
              <span>説明</span>
              <textarea
                rows={3}
                value={values.description}
                onChange={(e) => setValues((v) => ({ ...v, description: e.target.value }))}
              />
            </label>

            {dialogMode.kind === 'edit' ? (
              <dl className="session-form-modal__readonly">
                <dt>分離モード</dt>
                <dd>{dialogMode.session.mode === 'worktree' ? 'worktree 分離' : 'リポ直上'}</dd>
                <dt>ブランチ</dt>
                <dd>{dialogMode.session.branch ?? '—'}</dd>
                <dt>CLI</dt>
                <dd>
                  {dialogMode.session.cli_kind}
                  {dialogMode.session.cli_command !== null
                    ? ` (${dialogMode.session.cli_command})`
                    : ''}
                </dd>
                <dt />
                <dd className="session-form-modal__note">これらは作成時のみ設定できます</dd>
              </dl>
            ) : (
              <>
                <fieldset className="session-form-modal__radio-group">
                  <legend>分離モード</legend>
                  {MODES.map((mode) => (
                    <label key={mode} className="session-form-modal__radio">
                      <input
                        type="radio"
                        name="mode"
                        value={mode}
                        checked={values.mode === mode}
                        onChange={() => setValues((v) => ({ ...v, mode }))}
                      />
                      {mode === 'worktree' ? 'worktree 分離' : 'リポ直上'}
                    </label>
                  ))}
                </fieldset>

                {values.mode === 'worktree' ? (
                  <label className="session-form-modal__field session-form-modal__field--mono">
                    <span>ブランチ名</span>
                    <input
                      type="text"
                      value={values.branch}
                      placeholder="作成時に自動生成されます"
                      onChange={(e) => {
                        setBranchTouched(true);
                        setValues((v) => ({ ...v, branch: e.target.value }));
                      }}
                    />
                  </label>
                ) : null}

                <label className="session-form-modal__field">
                  <span>CLI</span>
                  <select
                    value={values.cliKind}
                    onChange={(e) =>
                      setValues((v) => ({ ...v, cliKind: e.target.value as CliKind }))
                    }
                  >
                    {CLI_KINDS.map((kind) => (
                      <option key={kind} value={kind}>
                        {kind}
                      </option>
                    ))}
                  </select>
                </label>

                {values.cliKind === 'custom' ? (
                  <label className="session-form-modal__field session-form-modal__field--mono">
                    <span>起動コマンド</span>
                    <input
                      type="text"
                      value={values.cliCommand}
                      placeholder="aider --model sonnet"
                      onChange={(e) => setValues((v) => ({ ...v, cliCommand: e.target.value }))}
                    />
                  </label>
                ) : null}
              </>
            )}

            {errors.length > 0 ? (
              <ul className="session-form-modal__errors">
                {errors.map((message) => (
                  <li key={message}>{message}</li>
                ))}
              </ul>
            ) : null}
          </div>

          <footer className="session-form-modal__footer">
            <button
              type="button"
              className="session-form-modal__button session-form-modal__button--ghost"
              onClick={closeModal}
            >
              キャンセル
            </button>
            <button
              type="submit"
              className="session-form-modal__button session-form-modal__button--primary"
              disabled={!canSubmit}
            >
              {isEdit ? '保存' : '作成'}
            </button>
          </footer>
        </form>
      </div>
    </div>
  );
}
