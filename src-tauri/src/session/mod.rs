// M2-1 がこのモジュールに SessionManager / runtime_state.rs を追加する
pub mod cli_args;

use tauri::{AppHandle, State};

use crate::error::{AppError, AppResult};
use crate::model::{Session, SurfaceKind};
use crate::pty::launch_env::LaunchEnv;
use crate::pty::{surface_id, SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};
use crate::state::AppState;
use cli_args::{build_launch_command, login_shell, resolve_cwd, ResumeMode};

/// `start_session` の `AppHandle` を必要としない部分だけを切り出した純関数。
///
/// 契約 §15 が `PtyManager::spawn(&self, app: &tauri::AppHandle, ..)` を Wry 固定で
/// 凍結しているため、`start_session` 全体は `MockRuntime` の `generate_handler!` に
/// 登録できず IPC テストから到達不能になる（フィックス対象レビュー指摘: Task 8 fix
/// round 1、Important 1）。`id -> get_session -> get_project -> resolve_cwd ->
/// build_launch_command -> SpawnSpec` の組み立て自体は `AppHandle` を要求しない
/// ので、ここへ分離すればランタイム不要の普通のユニットテストで固定できる。
/// `start_session` に残るのは `state.pty.spawn(&app, spec)` の 1 行だけになる。
fn plan_agent_spawn(state: &AppState, id: &str) -> AppResult<(Session, SpawnSpec)> {
    let session = state.store.get_session(id)?;
    let project = state.store.get_project(&session.project_id)?;
    let cwd = resolve_cwd(&session, &project.repo_path);

    // portable-pty 0.9.0 の `CommandBuilder::as_command()`（cmdbuilder.rs:501-506）は
    // cwd を `.filter(|dir| std::path::Path::new(dir).is_dir())` で検証し、ディレクトリ
    // でなければ chdir を一度も試みずに黙って $HOME へフォールバックする（stderr も出ない）。
    // したがって spawn 前のここでの検証だけが、この経路でユーザーに失敗を報告できる唯一の
    // 手段になる。述語は portable-pty の filter と逐語で一致させる必要があり、`exists()`
    // にすると通常ファイルの cwd がここを素通りして結局 $HOME に落ちる（直したつもりの
    // 再発）ため、`is_dir()` を使う。
    //
    // 申し送り（M1-4 への安全性）: M1-3 では UX の問題だが、M1-4 では安全性の問題になる。
    // M1-4 は同じ経路で `claude` を起動する。worktree の作成に失敗した状態で起動すると、
    // エージェントが $HOME で動き出し、無関係なファイルを触りうる。`resolve_cwd` の直後に
    // 置くのは、M1-4 が worktree を作ってから `start_session` を呼ぶ順序を壊さず、むしろ
    // worktree 作成の無言失敗を捕まえる網になるため。
    if !cwd.is_dir() {
        return Err(AppError::InvalidState(format!(
            "working directory does not exist or is not a directory: {}",
            cwd.display()
        )));
    }

    // M1-3 は shell のみなので program はログインシェル。
    // M1-4 はここを §18 の resolve_program(binary_name(cli_kind)?) に差し替え、
    // LaunchEnv::from_current_process() を probe_login_env() に差し替える。
    // shell の腕は launch_env を使わない（$SHELL -l が自分で PATH / LANG を作る、契約 §23）
    let launch_env = LaunchEnv::from_current_process();
    let launch = build_launch_command(
        &session,
        &login_shell(),
        &cwd,
        &launch_env,
        ResumeMode::None,
    )?;

    let spec = SpawnSpec {
        surface_id: surface_id(&session.id, SurfaceKind::Agent),
        program: launch.program.to_string_lossy().into_owned(),
        args: launch.args,
        cwd: launch.cwd,
        // KAMUX_SESSION_ID は build_launch_command が入れている
        env: launch.env,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    };

    Ok((session, spec))
}

/// セッションの agent サーフェスを起動する。
/// M1-3 では worktree 準備を行わない（M1-4 が start_session の前段に足す）。
/// サイズは 80x24 で起動し、フロントが attach 直後の fit() → resize_pty で合わせる。
#[tauri::command]
pub async fn start_session(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
) -> AppResult<Session> {
    let (session, spec) = plan_agent_spawn(&state, &id)?;
    state.pty.spawn(&app, spec)?;
    Ok(session)
}

/// agent サーフェスを殺す。runtime_state の遷移は M2-1 が担当する
#[tauri::command]
pub async fn stop_session(state: State<'_, AppState>, id: String) -> AppResult<Session> {
    let session = state.store.get_session(&id)?;
    state
        .pty
        .kill(&surface_id(&session.id, SurfaceKind::Agent))?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::model::{CliKind, SessionMode};
    use crate::pty::PtyManager;
    use crate::session::cli_args::tests::{ShellEnvGuard, ENV_LOCK};
    use crate::store::now_ms;
    use crate::store::test_support::open_temp;

    /// worktree モードのプロジェクト・セッションだけを作り、worktree の設定は呼び出し側に
    /// 委ねる。cwd 実在検証のテスト（存在しないパス／通常ファイル）が実体を持たない
    /// worktree_path を注入できるようにするための共通部分。
    fn build_project_and_session() -> (tempfile::TempDir, crate::store::Store, Session) {
        let (dir, store) = open_temp();
        let project = store
            .insert_project("kamux", "/tmp/kamux-test-repo", CliKind::Shell)
            .expect("insert project");
        let session = Session::new_backlog(
            &project.id,
            "shell session",
            "",
            SessionMode::Worktree,
            Some("session/shell".to_string()),
            CliKind::Shell,
            None,
            1.0,
            now_ms(),
        );
        let session = store.insert_session(&session).expect("insert session");
        (dir, store, session)
    }

    /// worktree セッションを 1 件持つ `AppState` を組み立てる。
    /// `worktree_path` を `repo_path` と明確に別値にするのは、`resolve_cwd` を経由した
    /// 値であることを、`project.repo_path` をそのまま使う変異と区別できるようにするため。
    /// `plan_agent_spawn` が cwd の実在（is_dir）を検証するようになったため、
    /// `/tmp/kamux-test-worktree` のような存在しない固定パスは使えない。`dir`（既存の
    /// TempDir）の中にサブディレクトリを作って渡す。2 個目の TempDir を作って渡さずに
    /// 落とすと即座に drop されて消えるため使わない。
    fn build_state_with_worktree_session() -> (tempfile::TempDir, AppState, Session, PathBuf) {
        let (dir, store, session) = build_project_and_session();
        let worktree_path = dir.path().join("worktree");
        std::fs::create_dir(&worktree_path).expect("create worktree dir");
        store
            .set_worktree(
                &session.id,
                "session/shell",
                worktree_path.to_str().expect("utf8 path"),
            )
            .expect("set worktree");
        let state = AppState {
            store: Arc::new(store),
            pty: PtyManager::new(),
        };
        (dir, state, session, worktree_path)
    }

    /// 変異 1: `surface_id(&session.id, SurfaceKind::Agent)` を `SurfaceKind::Editor`
    /// に変えると、この assert が赤くなる（`SpawnSpec.surface_id` の接尾辞が `:agent`
    /// であることの固定）。
    #[test]
    fn plan_agent_spawn_builds_the_agent_surface_id() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) = build_state_with_worktree_session();

        let (_, spec) = plan_agent_spawn(&state, &session.id).expect("plan spawn");

        assert_eq!(spec.surface_id, surface_id(&session.id, SurfaceKind::Agent));
    }

    /// 変異 2: `resolve_cwd(&session, &project.repo_path)` を `project.repo_path` の
    /// 素通しに変えると、この assert が赤くなる（`cwd` が `resolve_cwd` の戻り値である
    /// ことの固定）。`repo_path` と `worktree_path` を別値にしているのがこの弁別力の要。
    #[test]
    fn plan_agent_spawn_resolves_cwd_from_the_worktree_path_not_the_repo_path() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, worktree_path) = build_state_with_worktree_session();

        let (_, spec) = plan_agent_spawn(&state, &session.id).expect("plan spawn");

        assert_eq!(spec.cwd, worktree_path);
    }

    /// 変異 3: `build_launch_command` が入れる `KAMUX_SESSION_ID` を `spec.env` から
    /// 落とすと、この assert が赤くなる（Task 3 レビュー申し送り (d) と PR 8 の
    /// `SpawnSpec.env` 弁別テスト欠落指摘の着地点）。
    #[test]
    fn plan_agent_spawn_injects_kamux_session_id_into_env() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) = build_state_with_worktree_session();

        let (_, spec) = plan_agent_spawn(&state, &session.id).expect("plan spawn");

        assert!(spec
            .env
            .contains(&("KAMUX_SESSION_ID".to_string(), session.id.clone())));
    }

    /// 変異 4: `cols: DEFAULT_COLS` / `rows: DEFAULT_ROWS` を別の値に変えると、この
    /// assert が赤くなる（起動サイズが 80x24 であることの固定）。定数ではなくリテラル
    /// と比較するのは、定数側の変異と同時に緑へ戻ってしまうのを避けるため。
    #[test]
    fn plan_agent_spawn_starts_at_80x24() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) = build_state_with_worktree_session();

        let (_, spec) = plan_agent_spawn(&state, &session.id).expect("plan spawn");

        assert_eq!(spec.cols, 80);
        assert_eq!(spec.rows, 24);
    }

    /// `program` が `login_shell()` 由来のログインシェルであること、`args` が契約
    /// §23 の shell の腕の形（`-l` 一本）であることを固定する。`ShellEnvGuard` で
    /// 既定値と異なる値へ差し替えるので、フォールバック値と一致してしまう vacuous な
    /// assert にならない。
    #[test]
    fn plan_agent_spawn_uses_the_login_shell_as_program_with_a_login_flag() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = ShellEnvGuard::set("/tmp/kamux-test-login-shell");
        let (_dir, state, session, _worktree_path) = build_state_with_worktree_session();

        let (_, spec) = plan_agent_spawn(&state, &session.id).expect("plan spawn");

        assert_eq!(spec.program, "/tmp/kamux-test-login-shell");
        assert_eq!(spec.args, vec!["-l".to_string()]);
    }

    /// `resolve_cwd` が返す cwd がディレクトリとして実在しない場合、`plan_agent_spawn` は
    /// spawn 前に `AppError::InvalidState` を返す。portable-pty 0.9.0 の
    /// `CommandBuilder::as_command()`（cmdbuilder.rs:501-506）は cwd を
    /// `.filter(|dir| std::path::Path::new(dir).is_dir())` で検証し、ディレクトリで
    /// なければ黙って `$HOME` にフォールバックする。この経路は chdir を一度も試みず
    /// stderr も出さないため、spawn 前の検証だけがユーザーへの唯一の報告手段になる。
    /// message に該当パスが含まれることも確認する（メッセージが空になる変異を弁別する）。
    #[test]
    fn plan_agent_spawn_rejects_a_worktree_path_that_does_not_exist() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (dir, store, session) = build_project_and_session();
        let missing_path = dir.path().join("does-not-exist");
        store
            .set_worktree(
                &session.id,
                "session/shell",
                missing_path.to_str().expect("utf8 path"),
            )
            .expect("set worktree");
        let state = AppState {
            store: Arc::new(store),
            pty: PtyManager::new(),
        };

        let err = plan_agent_spawn(&state, &session.id).expect_err("must reject missing cwd");

        match err {
            AppError::InvalidState(message) => {
                assert!(
                    message.contains(missing_path.to_str().expect("utf8 path")),
                    "message does not mention the missing path: {message}"
                );
            }
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    /// cwd が「ディレクトリではない通常ファイル」の場合も同様に拒否する。portable-pty の
    /// `.filter(is_dir)` と述語を逐語で一致させる必要があるため `exists()` ではなく
    /// `is_dir()` を使う。この変異は `is_dir()` → `exists()` の入れ替えを弁別できる唯一
    /// のテスト（通常ファイルは `exists()` を通ってしまう）。
    #[test]
    fn plan_agent_spawn_rejects_a_worktree_path_that_is_a_regular_file() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (dir, store, session) = build_project_and_session();
        let file_path = dir.path().join("not-a-directory");
        std::fs::write(&file_path, b"").expect("create regular file");
        store
            .set_worktree(
                &session.id,
                "session/shell",
                file_path.to_str().expect("utf8 path"),
            )
            .expect("set worktree");
        let state = AppState {
            store: Arc::new(store),
            pty: PtyManager::new(),
        };

        let err =
            plan_agent_spawn(&state, &session.id).expect_err("must reject a regular file cwd");

        assert!(matches!(err, AppError::InvalidState(_)), "actual: {err:?}");
    }
}
