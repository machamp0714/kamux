// M2-1 がこのモジュールに SessionManager / runtime_state.rs を追加する
pub mod cli_args;

use tauri::{AppHandle, State};

use crate::error::AppResult;
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
    use std::sync::Arc;

    use super::*;
    use crate::model::{CliKind, SessionMode};
    use crate::pty::PtyManager;
    use crate::session::cli_args::tests::{ShellEnvGuard, ENV_LOCK};
    use crate::store::now_ms;
    use crate::store::test_support::open_temp;

    /// worktree セッションを 1 件持つ `AppState` を組み立てる。
    /// `worktree_path` を `repo_path` と明確に別値にするのは、`resolve_cwd` を経由した
    /// 値であることを、`project.repo_path` をそのまま使う変異と区別できるようにするため。
    /// `insert_test_session` ヘルパは `worktree_path: None` の in_place セッションしか
    /// 作れないので、ここでは `Session::new_backlog` を直接使う（`store/mod.rs` の
    /// `insert_test_session` ドキュメントコメントが明示する逃げ道）。
    fn build_state_with_worktree_session() -> (tempfile::TempDir, AppState, Session) {
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
        store
            .set_worktree(&session.id, "session/shell", "/tmp/kamux-test-worktree")
            .expect("set worktree");
        let state = AppState {
            store: Arc::new(store),
            pty: PtyManager::new(),
        };
        (dir, state, session)
    }

    /// 変異 1: `surface_id(&session.id, SurfaceKind::Agent)` を `SurfaceKind::Editor`
    /// に変えると、この assert が赤くなる（`SpawnSpec.surface_id` の接尾辞が `:agent`
    /// であることの固定）。
    #[test]
    fn plan_agent_spawn_builds_the_agent_surface_id() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session) = build_state_with_worktree_session();

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
        let (_dir, state, session) = build_state_with_worktree_session();

        let (_, spec) = plan_agent_spawn(&state, &session.id).expect("plan spawn");

        assert_eq!(
            spec.cwd,
            std::path::PathBuf::from("/tmp/kamux-test-worktree")
        );
    }

    /// 変異 3: `build_launch_command` が入れる `KAMUX_SESSION_ID` を `spec.env` から
    /// 落とすと、この assert が赤くなる（Task 3 レビュー申し送り (d) と PR 8 の
    /// `SpawnSpec.env` 弁別テスト欠落指摘の着地点）。
    #[test]
    fn plan_agent_spawn_injects_kamux_session_id_into_env() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session) = build_state_with_worktree_session();

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
        let (_dir, state, session) = build_state_with_worktree_session();

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
        let (_dir, state, session) = build_state_with_worktree_session();

        let (_, spec) = plan_agent_spawn(&state, &session.id).expect("plan spawn");

        assert_eq!(spec.program, "/tmp/kamux-test-login-shell");
        assert_eq!(spec.args, vec!["-l".to_string()]);
    }
}
