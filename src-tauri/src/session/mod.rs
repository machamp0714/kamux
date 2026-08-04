// M2-1 がこのモジュールに SessionManager / runtime_state.rs を追加する
pub mod cli_args;
pub mod workspace;

use std::path::PathBuf;

use tauri::{AppHandle, State};

use crate::error::{AppError, AppResult};
use crate::model::{KanbanStatus, Session, SessionPatch, SurfaceKind};
use crate::pty::launch_env::{probe_login_env, resolve_program, LaunchEnv};
use crate::pty::{surface_id, SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};
use crate::state::AppState;
use cli_args::{binary_name, build_launch_command, login_shell, ResumeMode};
use workspace::{apply_start_kanban_transition, prepare_worktree};

/// `start_session` の `AppHandle` を必要としない部分だけを切り出した純関数（注入版）。
///
/// 契約 §15 が `PtyManager::spawn(&self, app: &tauri::AppHandle, ..)` を Wry 固定で
/// 凍結しているため、`start_session` 全体は `MockRuntime` の `generate_handler!` に
/// 登録できず IPC テストから到達不能になる（フィックス対象レビュー指摘: Task 8 fix
/// round 1、Important 1）。`id -> get_session -> get_project -> prepare_worktree ->
/// build_launch_command -> SpawnSpec` の組み立て自体は `AppHandle` を要求しない
/// ので、ここへ分離すればランタイム不要の普通のユニットテストで固定できる。
///
/// `launch_env` / `resolve_program` を引数で注入できるのは、`probe_login_env()` /
/// `resolve_program()`（契約 §18）が実環境（`$SHELL -ilc` の実行・実 PATH 探索）に
/// 触れるため（契約 §60.4 の注入版 seam）。ここへ実装をベタ書きすると、この関数を
/// 呼ぶすべてのテストが実行環境の `$SHELL` と実 `claude` の有無に依存してしまい、
/// 契約 §14「実 `claude` は使わない」を満たせなくなる。`plan_agent_spawn`（下）が
/// 実環境版へ委譲する薄いラッパである。
fn plan_agent_spawn_with(
    state: &AppState,
    id: &str,
    launch_env: &LaunchEnv,
    // レビュー指摘 M-2: パラメータ名を `resolve_program` にすると、同名で import した
    // 本物の `crate::pty::launch_env::resolve_program` をシャドウし、関数内で
    // どちらを呼んでいるか読みにくくなる。注入されたクロージャだと分かる名前にする。
    resolve: impl Fn(&str) -> AppResult<PathBuf>,
) -> AppResult<(Session, SpawnSpec)> {
    let sid = surface_id(id, SurfaceKind::Agent);

    // 設計判断 8: worktree 作成などの副作用（git ブランチ作成・ディレクトリ作成）を
    // 起こす前に、既に生きているセッションの二重起動を弾く。`PtyManager::spawn` 自身も
    // 同じ排他を持つ（契約 §15）が、そちらは spawn 直前のチェックなので、ここで弾かなければ
    // 「worktree は新規に作られたが spawn は拒否される」という中途半端な副作用が起きる。
    if state.pty.is_alive(&sid) {
        return Err(AppError::InvalidState(format!(
            "session {id} is already running"
        )));
    }

    let mut session = state.store.get_session(id)?;
    let project = state.store.get_project(&session.project_id)?;

    // 実行ファイルを解決する（契約 §18。claude/codex 未検出はここで CliNotFound）。
    // `binary_name` は Claude / Codex だけ `Some` を返す。Shell と Custom は**両方**
    // `None` を返す —— Custom は `cli_args::build_launch_command` の Custom の腕が
    // `$SHELL -l -c "<cli_command>"` を組む（シェル経由起動）ため、Shell と同じ
    // ログインシェルへ意図的に相乗りさせている。`None` を「シェルだから」と早合点して
    // Custom を別経路に倒すと、Custom セッションが誤ったプログラムへ飛ぶ
    // （Task 7 レビューが名指しで警告した事故）。
    //
    // **`prepare_worktree`（下）より前に解決すること**（レビュー指摘 Important 2）。
    // ここが `prepare_worktree` より後ろだと、claude 未インストール環境で
    // 「worktree はディスク上に作られたが CliNotFound で spawn まで届かない」状態になる。
    // 判断 6 によりこの失敗時は DB を書かないため、次回の `start_session` はまた
    // `session.worktree_path == None` から `prepare_worktree` を呼び直す。worktree
    // モードのセッションは作成時に必ず `branch` が非 null になる
    // （`sessionForm.ts` が空欄なら `proposeBranchName(title)` を埋める）ため、
    // `prepare_worktree` の再利用腕にも `suggest_branch_name` の重複回避にも入らず、
    // 既存 branch のまま `create_worktree` を再実行して git の
    // 「branch already exists」で毎回失敗する —— claude 未インストールのユーザーが
    // 1 回起動を試みただけでそのセッションが恒久的に起動不能になる。
    let program = match binary_name(session.cli_kind) {
        Some(name) => resolve(name)?.display().to_string(),
        None => login_shell(),
    };

    // worktree モードなら必要に応じて worktree を作る/再利用し、in_place ならそのまま
    // repo_path を返す（契約 §13 / 設計判断 8）。`session.branch` / `session.worktree_path`
    // をここで in-memory に確定させる。DB への永続化は spawn 成功後（`commit_started_session`）
    // まで行わない（設計判断 6）。
    let cwd = prepare_worktree(&project, &mut session)?;

    // portable-pty 0.9.0 の `CommandBuilder::as_command()`（cmdbuilder.rs:502-507）は
    // cwd を `.filter(|dir| std::path::Path::new(dir).is_dir())` で検証し、ディレクトリ
    // でなければ `.unwrap_or(home.as_ref())`（cmdbuilder.rs:507）で chdir を一度も試みず
    // 黙って $HOME へフォールバックする（stderr も出ない）。したがって spawn 前のここでの
    // 検証だけが、この経路でユーザーに失敗を報告できる唯一の手段になる。述語は
    // portable-pty の filter と逐語で一致させる必要があり、`exists()` にすると通常ファイルの
    // cwd がここを素通りして結局 $HOME に落ちる（直したつもりの再発）ため `is_dir()` を使う。
    //
    // worktree モードでは `prepare_worktree` 自身が既に cwd の実在を保証している
    // （新規作成は git が、既存の再利用は `is_dir()` チェックが担う）ので、この安全網が
    // 実際に効くのは in_place モードで `project.repo_path` が消えている場合だけである
    // （`prepare_worktree` の in_place 腕は検証せずそのまま返すため）。
    if !cwd.is_dir() {
        return Err(AppError::InvalidState(format!(
            "working directory does not exist or is not a directory: {}",
            cwd.display()
        )));
    }

    // 起動コマンドを組み立てる（契約 §23 の純粋関数）。
    // M1-4 は常に ResumeMode::None。M2-4 が resume_session で他の値を渡す。
    let launch = build_launch_command(&session, &program, &cwd, launch_env, ResumeMode::None)?;

    let spec = SpawnSpec {
        surface_id: sid,
        program: launch.program.to_string_lossy().into_owned(),
        // KAMUX_SESSION_ID / PATH / LANG は build_launch_command が入れている。
        // ここでは push しない（契約 §23「呼び出し側は一切 push しない」）。
        env: launch.env,
        args: launch.args,
        cwd: launch.cwd,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    };

    Ok((session, spec))
}

/// `plan_agent_spawn_with` の実環境版。契約 §18 の公開 API（`probe_login_env` /
/// `resolve_program`）へ委譲する薄いラッパ（契約 §60.4）。
fn plan_agent_spawn(state: &AppState, id: &str) -> AppResult<(Session, SpawnSpec)> {
    plan_agent_spawn_with(state, id, probe_login_env(), resolve_program)
}

/// PTY spawn が成功した**後**にのみ呼ぶ。worktree の確定・カンバン列の遷移判定・
/// 永続化を行う（設計判断 6: 失敗時にカードが In Progress へ飛ぶと DB が嘘をつく）。
///
/// `AppHandle` も `PtyManager` も要らないため、実 PTY を起動せずに固定できる。
/// `start_session` に残る `AppHandle` 依存の行は `state.pty.spawn(&app, spec)` の
/// 1 行だけになる。
fn commit_started_session(state: &AppState, session: &mut Session) -> AppResult<Session> {
    // `Store::update_session` の SET 句は title/description/kanban_status/sort_order/
    // archived_at/updated_at だけで branch/worktree_path を含まない
    // （`session_dao.rs` 実測）。`prepare_worktree` が in-memory に確定させた値は
    // `set_worktree` を別途呼ばない限り DB に残らず、次回起動でまた新規 worktree を
    // 作ってしまう。worktree モードなら常に両方 Some、in_place なら常に両方 None
    // なので `if let` の両側同時成立だけを見ればよい。
    if let (Some(branch), Some(worktree_path)) = (&session.branch, &session.worktree_path) {
        state
            .store
            .set_worktree(&session.id, branch, worktree_path)?;
    }

    if session.kanban_status != KanbanStatus::Backlog {
        // 設計書 §5.3: 自動遷移は backlog -> in_progress のみ。review / done /
        // in_progress のセッションは列を動かさない。ここで `update_session` を
        // 呼ぶと、変更する値が無いのに `updated_at` だけが動く —— §38.2 が禁じる
        // 「ランタイム由来の書き込みで最終更新順を汚す」に当たるため呼ばない。
        // `next_sort_order` すら引かない。
        //
        // 戻り値は `session.clone()` ではなく `get_session` で読み直す。
        // `set_worktree`（上）は `updated_at` を自前で進めるため、fetch し直さないと
        // 返り値の `updated_at` が DB の値より古いまま返ってしまう。
        return state.store.get_session(&session.id);
    }

    let tail = state
        .store
        .next_sort_order(&session.project_id, KanbanStatus::InProgress)?;
    // 純関数。kanban_status == Backlog のときだけ InProgress へ遷移させる（設計判断 6）。
    // last_runtime_state は一切触らない（設計判断 7。M2-1 が pty://exit 購読で一元的に書く）。
    apply_start_kanban_transition(session, tail);

    // 契約 §17: 部分更新は SessionPatch で行う。branch/worktree_path は上で
    // set_worktree 済みなので、ここでは kanban_status / sort_order だけを書く。
    state.store.update_session(
        &session.id,
        &SessionPatch {
            kanban_status: Some(session.kanban_status),
            sort_order: Some(session.sort_order),
            ..Default::default()
        },
    )
}

/// セッションの agent サーフェスを起動する。
/// サイズは 80x24 で起動し、フロントが attach 直後の fit() → resize_pty で合わせる。
#[tauri::command]
pub async fn start_session(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
) -> AppResult<Session> {
    let (mut session, spec) = plan_agent_spawn(&state, &id)?;
    // AppHandle に依存する行はこの 1 行だけ（設計判断 6: spawn 成功後にのみ DB を書く）。
    // `state.pty.spawn` が Wry 固定（契約 §15）で MockRuntime に登録できないため、
    // 「spawn 成功後にのみ commit_started_session を呼ぶ」という順序はユニットテストでは
    // 担保できない。この 3 行の構造そのもの（`?` の早期リターンが commit の手前にあること）
    // を目視レビューで担保する（レビュー指摘 M-1）。
    state.pty.spawn(&app, spec)?;
    commit_started_session(&state, &mut session)
}

/// agent サーフェスを殺す。runtime_state の遷移は M2-1 が担当する（設計判断 7）。
/// `PtyManager::kill` は冪等（契約 §15）なので、事前の `is_alive` 確認は不要。
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
    use crate::model::{CliKind, RuntimeState, SessionMode};
    use crate::pty::surface::PtySink;
    use crate::pty::PtyManager;
    use crate::session::cli_args::tests::{ShellEnvGuard, ENV_LOCK};
    use crate::store::now_ms;
    use crate::store::test_support::open_temp;
    use crate::worktree::test_support::TestRepo;

    /// テスト固定の `LaunchEnv`。実 `$SHELL -ilc` を一切踏まない（契約 §14）。
    fn fake_launch_env() -> LaunchEnv {
        LaunchEnv {
            path: "/fake/bin".to_string(),
            lang: "ja_JP.UTF-8".to_string(),
        }
    }

    /// テスト固定の `resolve_program`。実 PATH 探索・実 `claude` に一切触れない
    /// （契約 §14「実 claude は使わない」/ §60.4 の注入版 seam）。
    fn fake_resolve_program(program: &str) -> AppResult<PathBuf> {
        Ok(PathBuf::from(format!("/fake/bin/{program}")))
    }

    /// worktree モードのプロジェクト・セッションだけを作り、worktree の設定は呼び出し側に
    /// 委ねる。`mode` / `cli_kind` / `cli_command` を引数化しているのは、claude/codex/custom
    /// の分岐と in_place の cwd 検証を同じヘルパで組み立てられるようにするため
    /// （advisor 指摘: 既存ヘルパは Worktree/Shell 固定だった）。
    fn build_project_and_session(
        repo_path: &str,
        mode: SessionMode,
        cli_kind: CliKind,
        cli_command: Option<&str>,
    ) -> (tempfile::TempDir, crate::store::Store, Session) {
        let (dir, store) = open_temp();
        let project = store
            .insert_project("kamux", repo_path, CliKind::Shell)
            .expect("insert project");
        let session = Session::new_backlog(
            &project.id,
            "shell session",
            "",
            mode,
            Some("session/shell".to_string()),
            cli_kind,
            cli_command.map(|s| s.to_string()),
            1.0,
            now_ms(),
        );
        let session = store.insert_session(&session).expect("insert session");
        (dir, store, session)
    }

    /// `build_project_and_session` の結果を `AppState` に包むだけの薄いラッパー。
    /// `set_worktree` を一切呼ばないため、`session.worktree_path` は `None` のまま
    /// （in_place 相当）になる。「`repo_path` そのものが存在しないプロジェクト」を
    /// スモークで報告された症状のまま再現するテスト専用。
    fn build_state_without_worktree(
        repo_path: &str,
        mode: SessionMode,
    ) -> (tempfile::TempDir, AppState, Session) {
        let (dir, store, session) =
            build_project_and_session(repo_path, mode, CliKind::Shell, None);
        let state = AppState {
            store: Arc::new(store),
            pty: PtyManager::new(),
        };
        (dir, state, session)
    }

    /// worktree セッションを 1 件持つ `AppState` を組み立てる。
    /// `worktree_path` を `repo_path` と明確に別値にするのは、`prepare_worktree` の
    /// 再利用腕を経由した値であることを、`project.repo_path` をそのまま使う変異と
    /// 区別できるようにするため。実 git worktree は作らない（`is_dir()` チェックだけの
    /// 再利用腕を通るため、`TestRepo` 無しでも安全に再利用できる）。
    fn build_state_with_worktree_session(
        cli_kind: CliKind,
        cli_command: Option<&str>,
    ) -> (tempfile::TempDir, AppState, Session, PathBuf) {
        let (dir, store, session) = build_project_and_session(
            "/tmp/kamux-test-repo",
            SessionMode::Worktree,
            cli_kind,
            cli_command,
        );
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

    fn plan(state: &AppState, id: &str) -> AppResult<(Session, SpawnSpec)> {
        plan_agent_spawn_with(state, id, &fake_launch_env(), fake_resolve_program)
    }

    // ---- plan_agent_spawn_with ----

    /// 変異 1: `surface_id(&session.id, SurfaceKind::Agent)` を `SurfaceKind::Editor`
    /// に変えると、この assert が赤くなる（`SpawnSpec.surface_id` の接尾辞が `:agent`
    /// であることの固定）。
    #[test]
    fn plan_agent_spawn_builds_the_agent_surface_id() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Shell, None);

        let (_, spec) = plan(&state, &session.id).expect("plan spawn");

        assert_eq!(spec.surface_id, surface_id(&session.id, SurfaceKind::Agent));
    }

    /// 変異 2: `prepare_worktree` の戻り値を `project.repo_path` の素通しに変えると、
    /// この assert が赤くなる（`cwd` が `prepare_worktree` の戻り値であることの固定）。
    /// `repo_path`（"/tmp/kamux-test-repo"）は実在しないため、変異後は `cwd.is_dir()`
    /// の検証で弾かれて `Err(InvalidState)` になり、後続の `assert_eq!` ではなく
    /// `.expect("plan spawn")` のパニックとして捕まる。`repo_path` と `worktree_path`
    /// を別値にしているのがこの弁別力の要。
    #[test]
    fn plan_agent_spawn_resolves_cwd_from_prepare_worktree_not_the_repo_path() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, worktree_path) =
            build_state_with_worktree_session(CliKind::Shell, None);

        let (_, spec) = plan(&state, &session.id).expect("plan spawn");

        assert_eq!(spec.cwd, worktree_path);
    }

    /// 変異 3: `build_launch_command` が入れる `KAMUX_SESSION_ID` を `spec.env` から
    /// 落とすと、この assert が赤くなる。
    #[test]
    fn plan_agent_spawn_injects_kamux_session_id_into_env() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Shell, None);

        let (_, spec) = plan(&state, &session.id).expect("plan spawn");

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
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Shell, None);

        let (_, spec) = plan(&state, &session.id).expect("plan spawn");

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
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Shell, None);

        let (_, spec) = plan(&state, &session.id).expect("plan spawn");

        assert_eq!(spec.program, "/tmp/kamux-test-login-shell");
        assert_eq!(spec.args, vec!["-l".to_string()]);
    }

    /// 申し送り #1: `binary_name(CliKind::Claude)` は `Some("claude")` なので、
    /// `program` は注入した `resolve_program` の結果になる（`login_shell()` ではない）。
    /// 変異でこの分岐を消す（常に login_shell を使う）と `/fake/bin/claude` の代わりに
    /// `ShellEnvGuard` が返す固定値が出て、この assert が赤くなる。
    ///
    /// `PATH` の assert は `plan_agent_spawn_with` の `launch_env` 引数が実際に
    /// `build_launch_command` まで通っていることを固定する。`/fake/bin` は実行環境の
    /// 実 PATH と絶対に一致しないため、`&LaunchEnv::from_current_process()` などの
    /// 実環境値へ差し替える変異が入っても vacuous にならない。
    #[test]
    fn plan_agent_spawn_resolves_the_binary_via_resolve_program_for_claude() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = ShellEnvGuard::set("/tmp/kamux-test-login-shell");
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Claude, None);

        let (_, spec) = plan(&state, &session.id).expect("plan spawn");

        assert_eq!(spec.program, "/fake/bin/claude");
        assert!(
            spec.env
                .iter()
                .any(|(k, v)| k == "PATH" && v == "/fake/bin"),
            "injected launch_env must flow into the SpawnSpec env: {:?}",
            spec.env
        );
        // レビュー指摘 M-4: PATH だけ固定して LANG は無防備だった。契約 §18 が LANG を
        // 注入する目的（空のままだと nvim/claude で日本語ファイル名が化ける）を守れて
        // いることも合わせて固定する。
        assert!(
            spec.env
                .iter()
                .any(|(k, v)| k == "LANG" && v == "ja_JP.UTF-8"),
            "injected launch_env.lang must flow into the SpawnSpec env: {:?}",
            spec.env
        );
    }

    /// 申し送り #1 の核心: `binary_name` は `Shell` と `Custom` の**両方**で `None` を
    /// 返す。`None` を「シェルだから」と早合点して Custom を別経路に倒す変異
    /// （例: Custom を CliNotFound にする、または resolve_program を経由させる）を
    /// 弁別する。`program` は login_shell() 由来、`args` は cli_args.rs の Custom の
    /// 腕（`-l -c "<command>"`）であることの両方を固定する。
    #[test]
    fn plan_agent_spawn_uses_the_login_shell_for_custom_cli_kind_not_resolve_program() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = ShellEnvGuard::set("/tmp/kamux-test-login-shell");
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Custom, Some("echo hi"));

        let (_, spec) = plan(&state, &session.id).expect("plan spawn");

        assert_eq!(spec.program, "/tmp/kamux-test-login-shell");
        assert_eq!(
            spec.args,
            vec!["-l".to_string(), "-c".to_string(), "echo hi".to_string()]
        );
    }

    /// レビュー指摘 Important 2: バイナリ解決（`resolve`）は `prepare_worktree` より
    /// **前**に呼ばれなければならない。順序が逆だと、claude 未インストール環境で
    /// worktree ディレクトリと git ブランチだけがディスク上に作られたまま
    /// `AppError::CliNotFound` で失敗する。判断 6 によりこの失敗時は DB を書かないため、
    /// 次回の `start_session` はまた `worktree_path == None` から `prepare_worktree` を
    /// 呼び直す。worktree モードのセッションは作成時に必ず `branch` が非 null になる
    /// （`sessionForm.ts` が空欄なら `proposeBranchName(title)` を埋める）ため、
    /// 再利用腕にも `suggest_branch_name` の重複回避にも入らず、既存 branch のまま
    /// `create_worktree` を再実行して git の「branch already exists」で毎回失敗する
    /// —— claude 未インストールのユーザーが 1 回起動を試みただけでそのセッションが
    /// 恒久的に起動不能になる。`.worktrees/` が作られていないことまで確認することで、
    /// 順序の入れ替えを弁別する（`CliNotFound` が返ることだけを見るテストでは、
    /// 「worktree を作ってから失敗した」場合と区別できない）。
    #[test]
    fn plan_agent_spawn_does_not_create_a_worktree_when_the_binary_cannot_be_resolved() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let repo = TestRepo::new();
        let (_dir, store) = open_temp();
        let project = store
            .insert_project("kamux", repo.path().to_str().expect("utf8"), CliKind::Shell)
            .expect("insert project");
        let session = Session::new_backlog(
            &project.id,
            "fix login bug",
            "",
            SessionMode::Worktree,
            None,
            CliKind::Claude,
            None,
            1.0,
            now_ms(),
        );
        let session = store.insert_session(&session).expect("insert session");
        let state = AppState {
            store: Arc::new(store),
            pty: PtyManager::new(),
        };

        let failing_resolve =
            |_: &str| -> AppResult<PathBuf> { Err(AppError::CliNotFound("claude".to_string())) };
        let err = plan_agent_spawn_with(&state, &session.id, &fake_launch_env(), failing_resolve)
            .expect_err("must fail when the binary cannot be resolved");

        assert!(matches!(err, AppError::CliNotFound(_)), "actual: {err:?}");
        assert!(
            !repo.path().join(".worktrees").exists(),
            "worktree creation must not happen before the binary is resolved"
        );
    }

    /// `prepare_worktree` が既存 worktree の消失を `AppError::Git` として返す
    /// （workspace.rs の `missing_worktree_dir_errors_instead_of_recreating` で
    /// 個別に固定済み）。ここでは `plan_agent_spawn_with` がその Err をそのまま `?`
    /// で透過させ、`AppError::InvalidState` へすり替えたりしないことだけを固定する。
    #[test]
    fn plan_agent_spawn_propagates_git_errors_from_prepare_worktree() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (dir, store, session) = build_project_and_session(
            "/tmp/kamux-test-repo",
            SessionMode::Worktree,
            CliKind::Shell,
            None,
        );
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

        let err = plan(&state, &session.id).expect_err("must propagate the Git error");

        assert!(matches!(err, AppError::Git(_)), "actual: {err:?}");
    }

    /// スモークで実際に報告されたケース: `repo_path` がそもそも存在しない in_place
    /// セッションを起動する。`prepare_worktree` の in_place 腕は cwd の実在を検証
    /// しない（そのまま `repo_path` を返す）ため、`plan_agent_spawn_with` 自身の
    /// `is_dir()` 安全網だけがこれを捕まえる —— refactor 後、この安全網を守っている
    /// のはこのテストだけである。
    #[test]
    fn plan_agent_spawn_rejects_a_missing_repo_path_for_in_place_sessions() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session) = build_state_without_worktree(
            "/tmp/kamux-test-repo-in-place-missing",
            SessionMode::InPlace,
        );

        let err = plan(&state, &session.id).expect_err("must reject missing repo_path");

        match err {
            AppError::InvalidState(message) => {
                assert!(
                    message.contains("/tmp/kamux-test-repo-in-place-missing"),
                    "message does not mention the missing repo_path: {message}"
                );
            }
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    /// 設計判断 8 / 順序の制約 4: 既に PTY が生きているセッションは、worktree 作成
    /// などの副作用を起こす前に `AppError::InvalidState` で弾かれる。`.worktrees/`
    /// が作られていないことまで確認することで、早期リターンが本当に副作用の手前に
    /// あることを固定する（ガードを `prepare_worktree` の後ろへ動かす変異は、
    /// エラー型は同じでも `.worktrees/` が作られてしまい、この assert が弁別する）。
    #[test]
    fn plan_agent_spawn_rejects_a_double_start_before_touching_the_worktree() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let repo = TestRepo::new();
        let (_dir, store) = open_temp();
        let project = store
            .insert_project("kamux", repo.path().to_str().expect("utf8"), CliKind::Shell)
            .expect("insert project");
        let session = Session::new_backlog(
            &project.id,
            "fix login bug",
            "",
            SessionMode::Worktree,
            None,
            CliKind::Shell,
            None,
            1.0,
            now_ms(),
        );
        let session = store.insert_session(&session).expect("insert session");
        let state = AppState {
            store: Arc::new(store),
            pty: PtyManager::new(),
        };
        let sid = surface_id(&session.id, SurfaceKind::Agent);

        struct NoopSink;
        impl PtySink for NoopSink {
            fn on_data(&self, _surface_id: &str, _base64: String, _seq: u64) {}
            fn on_exit(&self, _surface_id: &str, _exit_code: Option<i32>) {}
        }
        state
            .pty
            .spawn_with_sink(
                Arc::new(NoopSink),
                SpawnSpec {
                    surface_id: sid.clone(),
                    program: "/bin/cat".to_string(),
                    args: Vec::new(),
                    cwd: PathBuf::from("/tmp"),
                    env: Vec::new(),
                    cols: DEFAULT_COLS,
                    rows: DEFAULT_ROWS,
                },
            )
            .expect("spawn stub surface");

        let err = plan(&state, &session.id).expect_err("must reject the double start");

        assert!(matches!(err, AppError::InvalidState(_)), "actual: {err:?}");
        assert!(
            !repo.path().join(".worktrees").exists(),
            "worktree creation must not happen before the is_alive guard"
        );

        state.pty.kill(&sid).expect("cleanup stub surface");
    }

    // ---- commit_started_session ----

    fn build_state(repo_path: &str) -> (tempfile::TempDir, AppState, crate::model::Project) {
        let (dir, store) = open_temp();
        let project = store
            .insert_project("kamux", repo_path, CliKind::Shell)
            .expect("insert project");
        let state = AppState {
            store: Arc::new(store),
            pty: PtyManager::new(),
        };
        (dir, state, project)
    }

    /// 申し送り #3 の直接の網: `commit_started_session` が `set_worktree` を呼ばず
    /// `update_session` だけに戻る変異を入れると、再読み込みした `branch` /
    /// `worktree_path` が `None` のままになりこの assert が赤くなる。
    #[test]
    fn commit_started_session_persists_branch_and_worktree_path() {
        let repo = TestRepo::new();
        let (_dir, state, project) = build_state(repo.path().to_str().expect("utf8"));
        let sort_order = state
            .store
            .next_sort_order(&project.id, KanbanStatus::Backlog)
            .expect("next_sort_order");
        let session = Session::new_backlog(
            &project.id,
            "Fix login bug",
            "",
            SessionMode::Worktree,
            None,
            CliKind::Shell,
            None,
            sort_order,
            now_ms(),
        );
        let mut session = state
            .store
            .insert_session(&session)
            .expect("insert session");
        prepare_worktree(&project, &mut session).expect("prepare worktree");
        assert!(session.branch.is_some());
        assert!(session.worktree_path.is_some());

        commit_started_session(&state, &mut session).expect("commit");

        let reloaded = state.store.get_session(&session.id).expect("get_session");
        assert_eq!(reloaded.branch, session.branch);
        assert_eq!(reloaded.worktree_path, session.worktree_path);
    }

    /// 順序の制約 2 を DB 行のレベルで固定する。既存の in_progress セッションを 1 件
    /// 仕込んで tail（採番される sort_order）が現在の値と一致しない状況を作ることに加え、
    /// **Backlog 列に sort_order の大きいダミーカードを置く**（レビュー指摘 Important 1）。
    ///
    /// この decoy が無いと、テスト対象セッション自身が Backlog 列の唯一のカード
    /// （sort_order ≒ 1.0）になるため、`next_sort_order` に渡す `KanbanStatus` を
    /// `InProgress` → `Backlog` に取り違える変異を入れても、Backlog 列の
    /// `COALESCE(MAX(sort_order), 0) + 1` がたまたま InProgress 列の正しい tail と
    /// 同じ値（2.0）を返してしまい、変異が検出できなかった（実測: 修正前のこのテストは
    /// 変異を入れても 273 件全部緑のまま通っていた）。decoy の sort_order を 100.0 まで
    /// 押し上げることで、取り違えた場合の値（≒101.0）と正しい値（≒2.0）を大きく引き離す。
    #[test]
    fn commit_started_session_moves_a_backlog_session_to_the_tail_of_in_progress() {
        let (_dir, state, project) = build_state("/tmp/kamux-test-repo");
        let existing_sort = state
            .store
            .next_sort_order(&project.id, KanbanStatus::InProgress)
            .expect("next_sort_order");
        let existing = Session::new_backlog(
            &project.id,
            "already running",
            "",
            SessionMode::InPlace,
            None,
            CliKind::Shell,
            None,
            existing_sort,
            now_ms(),
        );
        let existing = state
            .store
            .insert_session(&existing)
            .expect("insert existing");
        // `Session::new_backlog` は kanban_status を常に Backlog で作る。InProgress の
        // 既存カードを再現するには挿入後に列を動かす必要がある
        // （そうしないと下の next_sort_order(InProgress) が existing を見つけられず、
        // tail が偶然 backlog_sort と一致して vacuous になる）。
        state
            .store
            .update_session(
                &existing.id,
                &SessionPatch {
                    kanban_status: Some(KanbanStatus::InProgress),
                    ..Default::default()
                },
            )
            .expect("move existing to in_progress");

        // Backlog 列の decoy: sort_order を大きく引き離すことで、next_sort_order に
        // 渡す KanbanStatus の取り違えを弁別できるようにする（上記コメント参照）。
        let decoy_sort = state
            .store
            .next_sort_order(&project.id, KanbanStatus::Backlog)
            .expect("next_sort_order");
        let decoy = Session::new_backlog(
            &project.id,
            "decoy backlog card",
            "",
            SessionMode::InPlace,
            None,
            CliKind::Shell,
            None,
            decoy_sort,
            now_ms(),
        );
        let decoy = state.store.insert_session(&decoy).expect("insert decoy");
        state
            .store
            .update_session(
                &decoy.id,
                &SessionPatch {
                    sort_order: Some(100.0),
                    ..Default::default()
                },
            )
            .expect("push decoy sort_order high");

        let backlog_sort = state
            .store
            .next_sort_order(&project.id, KanbanStatus::Backlog)
            .expect("next_sort_order");
        let session = Session::new_backlog(
            &project.id,
            "fix login bug",
            "",
            SessionMode::InPlace,
            None,
            CliKind::Shell,
            None,
            backlog_sort,
            now_ms(),
        );
        let mut session = state
            .store
            .insert_session(&session)
            .expect("insert session");

        let result = commit_started_session(&state, &mut session).expect("commit");

        assert_eq!(result.kanban_status, KanbanStatus::InProgress);
        // 正しい実装なら In Progress 列の tail（existing_sort のすぐ後ろ）になり、
        // Backlog 列の decoy（sort_order=100.0）には一切影響されない。取り違え変異が
        // 入ると decoy に引きずられて 101.0 付近まで跳ね上がるため、上限 10.0 で弁別する。
        assert!(
            result.sort_order > existing_sort && result.sort_order < 10.0,
            "In Progress 列の末尾（既存カードのすぐ後ろ）に置かれるべきで、Backlog 列の \
             decoy（100.0）には影響されないはず: {} (existing_sort={})",
            result.sort_order,
            existing_sort
        );
        let reloaded = state.store.get_session(&session.id).expect("get_session");
        assert_eq!(reloaded.kanban_status, KanbanStatus::InProgress);
        assert_eq!(reloaded.sort_order, result.sort_order);
    }

    /// 順序の制約 2: review / done / in_progress のセッションは列を動かさない。
    /// in_place セッション（`set_worktree` の腕を一度も通らない）で検証することで、
    /// 「遷移しないのに `update_session` を呼んでしまい `updated_at` だけ動く」変異
    /// （§38.2 が禁じる形）を、`set_worktree` 自身の `updated_at` 更新と混同せずに
    /// 弁別する。
    #[test]
    fn commit_started_session_does_not_advance_a_review_session_or_touch_updated_at() {
        let (_dir, state, project) = build_state("/tmp/kamux-test-repo");
        let sort_order = state
            .store
            .next_sort_order(&project.id, KanbanStatus::Review)
            .expect("next_sort_order");
        let session = Session::new_backlog(
            &project.id,
            "in review",
            "",
            SessionMode::InPlace,
            None,
            CliKind::Shell,
            None,
            sort_order,
            now_ms(),
        );
        let inserted = state
            .store
            .insert_session(&session)
            .expect("insert session");
        let mut session = state
            .store
            .update_session(
                &inserted.id,
                &SessionPatch {
                    kanban_status: Some(KanbanStatus::Review),
                    ..Default::default()
                },
            )
            .expect("move to review");
        let before = session.clone();

        let result = commit_started_session(&state, &mut session).expect("commit");

        assert_eq!(result.kanban_status, KanbanStatus::Review);
        assert_eq!(result.sort_order, before.sort_order);
        let reloaded = state.store.get_session(&session.id).expect("get_session");
        assert_eq!(
            reloaded.updated_at, before.updated_at,
            "遷移しないセッションで updated_at が動いてはならない（§38.2）"
        );
    }

    /// 回帰テスト: 非遷移パスの戻り値は `session.clone()`（呼び出し前のスナップショット）
    /// ではなく、DB を読み直した最新行でなければならない。worktree モードの Review
    /// セッション（既に `set_worktree` 済み）で検証する —— `commit_started_session` は
    /// 非遷移パスでも `set_worktree` を呼び直すため（現在値の再確認）、その呼び出しが
    /// `updated_at` を進める。`clone()` のまま実装すると、返り値の `updated_at` が
    /// 呼び出し前の（古い）値のままになりこの assert が赤くなる。
    ///
    /// `now_ms()` はミリ秒精度なので、直前の `update_session`（列を Review へ移す）と
    /// `commit_started_session` 内の `set_worktree` が同じミリ秒内で走ると、バグ入りの
    /// `clone()` でも偶然 `updated_at` が一致して緑になってしまう（実測済み: sleep 無しで
    /// 変異検証したところ検出できなかった）。決定的に弁別するため、2 回の書き込みの間に
    /// 短い sleep を挟んでミリ秒境界を跨がせる。
    #[test]
    fn commit_started_session_returns_the_reloaded_row_not_a_stale_clone_when_not_advancing() {
        let (_dir, state, project) = build_state("/tmp/kamux-test-repo");
        let sort_order = state
            .store
            .next_sort_order(&project.id, KanbanStatus::Review)
            .expect("next_sort_order");
        let session = Session::new_backlog(
            &project.id,
            "in review",
            "",
            SessionMode::Worktree,
            None,
            CliKind::Shell,
            None,
            sort_order,
            now_ms(),
        );
        let inserted = state
            .store
            .insert_session(&session)
            .expect("insert session");
        state
            .store
            .set_worktree(
                &inserted.id,
                "session/in-review",
                "/tmp/kamux-test-repo/.worktrees/in-review",
            )
            .expect("seed worktree");
        let mut session = state
            .store
            .update_session(
                &inserted.id,
                &SessionPatch {
                    kanban_status: Some(KanbanStatus::Review),
                    ..Default::default()
                },
            )
            .expect("move to review");
        let updated_at_before_commit = session.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(5));

        let result = commit_started_session(&state, &mut session).expect("commit");

        let reloaded = state.store.get_session(&session.id).expect("get_session");
        assert_eq!(
            result.updated_at, reloaded.updated_at,
            "戻り値は DB の最新行と一致しなければならない（clone() ではなく get_session 由来）"
        );
        assert!(
            result.updated_at > updated_at_before_commit,
            "commit_started_session 内の set_worktree が進めた updated_at が戻り値に反映されていない"
        );
        assert_eq!(result.branch, reloaded.branch);
        assert_eq!(result.worktree_path, reloaded.worktree_path);
    }

    /// 判断 7 / 契約 §2: `commit_started_session` は `last_runtime_state` を一切
    /// 書かない。事前に `Running` を書いておき、backlog -> in_progress の遷移が
    /// 実際に起きるセッションで呼んでも値が変わらないことを固定する
    /// （遷移しないセッションだけで確認すると `commit_started_session` が
    /// `update_session` すら呼ばない早期リターンで vacuous になるため、
    /// 遷移するケースで検証する）。
    #[test]
    fn commit_started_session_never_writes_runtime_state() {
        let (_dir, state, project) = build_state("/tmp/kamux-test-repo");
        let sort_order = state
            .store
            .next_sort_order(&project.id, KanbanStatus::Backlog)
            .expect("next_sort_order");
        let session = Session::new_backlog(
            &project.id,
            "fix login bug",
            "",
            SessionMode::InPlace,
            None,
            CliKind::Shell,
            None,
            sort_order,
            now_ms(),
        );
        let mut session = state
            .store
            .insert_session(&session)
            .expect("insert session");
        state
            .store
            .set_last_runtime_state(&session.id, RuntimeState::Running)
            .expect("seed running");

        commit_started_session(&state, &mut session).expect("commit");

        let reloaded = state.store.get_session(&session.id).expect("get_session");
        assert_eq!(
            reloaded.last_runtime_state,
            RuntimeState::Running,
            "M1-4 は last_runtime_state を書いてはならない（判断 7）"
        );
    }
}
