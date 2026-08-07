use std::path::PathBuf;

use tauri::{AppHandle, State};

use crate::error::{AppError, AppResult};
use crate::model::SurfaceKind;
use crate::pty::launch_env::{probe_login_env, resolve_program, LaunchEnv};
use crate::pty::{surface_id, SpawnSpec};
use crate::session::cli_args::resolve_cwd;
use crate::state::AppState;

/// ログインシェルのロケールが UTF-8 でないときの既定。
pub const EDITOR_FALLBACK_LANG: &str = "en_US.UTF-8";

// EDITOR_TERM / EDITOR_COLORTERM は**置かない**(契約 §60.6 / §60.8)。
// TERM / COLORTERM はここに入れない。所有は `PtySurface::spawn`(契約 §60.6.1 / §60.6.2)。
// あちらの env 設定行の隣に 1 行ずつ並ぶ。ここに定数を置くと真実源が 2 箇所になり、
// spec.env はその設定より後に適用される(spec.env を書き込むループがそれより後にある)
// ぶん、契約に行を持たない側が黙って勝つ。
// 検査 10(契約 §60.6.4)が「TERM / COLORTERM を設定するファイルはちょうど 1 つ」を見ている。

/// nvim 用 PTY に注入する環境変数(契約 §15: 既存環境を継承した上で上書きされる)。
///
/// PATH と LANG は契約 §18 の `probe_login_env()` が探査したもの。M3-1 は探査しない。
/// KAMUX_SESSION_ID は意図的に入れない(設計判断 D11)。
/// TERM / COLORTERM は入れない(契約 §60.6。所有は `PtySurface::spawn`)。
///
/// LANG の UTF-8 判定を残しているのは、§18 のシステムロケール導出が働くのが
/// 「探査値が空のとき」だけで、LANG=C を明示している環境では C がそのまま来るため
/// (C ロケールの nvim は日本語ファイル名・日本語テキストを化けさせる。設計判断 D6)。
pub fn build_editor_env(launch: &LaunchEnv) -> Vec<(String, String)> {
    let lang = if launch
        .lang
        .to_ascii_uppercase()
        .replace('-', "")
        .contains("UTF8")
    {
        launch.lang.clone()
    } else {
        EDITOR_FALLBACK_LANG.to_string()
    };
    vec![
        ("PATH".to_string(), launch.path.clone()),
        ("LANG".to_string(), lang),
    ]
}

/// 同時に live にできる nvim サーフェスの上限(契約 §19 / 設計判断 D3)。
/// LRU で自動 kill しないのは、未保存バッファを黙って捨てないため。
pub const MAX_LIVE_EDITOR_SURFACES: usize = 3;

/// 契約 §5 の surface_id 形式 "{session_id}:{surface_kind}" に対する判定。
pub fn is_editor_surface(surface_id: &str) -> bool {
    surface_id.ends_with(":editor")
}

pub fn count_live_editor_surfaces(surface_ids: &[String]) -> usize {
    surface_ids.iter().filter(|s| is_editor_surface(s)).count()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorSpawnDecision {
    AlreadyLive,
    Spawn,
    LimitReached,
}

/// live は `PtyManager::live_surfaces()` の戻り値(契約 §15)。
pub fn decide_editor_spawn(target: &str, live: &[String], max: usize) -> EditorSpawnDecision {
    if live.iter().any(|s| s == target) {
        return EditorSpawnDecision::AlreadyLive;
    }
    if count_live_editor_surfaces(live) >= max {
        return EditorSpawnDecision::LimitReached;
    }
    EditorSpawnDecision::Spawn
}

/// `spawn_editor` が `AppHandle` に触れる前に確定できる計画。
///
/// `AlreadyLive` なら呼び出し側は spawn せず同じ surface_id を返す。`Spawn` なら
/// 呼び出し側が `state.pty.spawn(&app, spec)` を呼ぶ。この分割は `session/mod.rs` の
/// `plan_agent_spawn_with` と同じ理由による —— 契約 §15 の `PtyManager::spawn` は
/// `AppHandle<Wry>` 固定で `MockRuntime` から呼べないため、それより前の組み立てだけを
/// 切り出せばユニットテストで固定できる。§60.4 が M3-1 向けに名指しした注入版 seam は
/// `resolve_program_in` / `probe_login_env_with`(§18 の追補)であり、この `plan_*_with`
/// 形式そのものではない —— ここで §60.4 に従っているのは「フェーズ境界を越えて
/// 参照されないため非公開に留める」という原則の部分であり、`session/mod.rs:33` の
/// `plan_agent_spawn_with` と同形の DI を plan 層に置いた、というのが正確な言い方。
#[derive(Debug)]
enum EditorSpawnPlan {
    AlreadyLive(String),
    Spawn(SpawnSpec),
}

/// `spawn_editor` の `AppHandle` を必要としない部分だけを切り出した純関数(注入版)。
/// `launch` / `resolve` を注入できる理由は `plan_agent_spawn_with` と同じ
/// (契約 §18 の実環境探査に実テストを引きずり込まないため)。
fn plan_editor_spawn_with(
    state: &AppState,
    session_id: &str,
    launch: &LaunchEnv,
    resolve: impl Fn(&str) -> AppResult<PathBuf>,
) -> AppResult<EditorSpawnPlan> {
    let sid = surface_id(session_id, SurfaceKind::Editor);

    match decide_editor_spawn(&sid, &state.pty.live_surfaces(), MAX_LIVE_EDITOR_SURFACES) {
        EditorSpawnDecision::AlreadyLive => return Ok(EditorSpawnPlan::AlreadyLive(sid)),
        EditorSpawnDecision::LimitReached => {
            return Err(AppError::InvalidState(format!(
                "editor limit reached: at most {MAX_LIVE_EDITOR_SURFACES} nvim instances can be open at once. \
                 run :qa in one of them to free a slot"
            )))
        }
        EditorSpawnDecision::Spawn => {}
    }

    // decide_editor_spawn は最初のゲートに過ぎない。EditorSurface は再マウントのたびに
    // spawn_editor を呼ぶため、同一セッションに対する 2 本の呼び出しが同時に Spawn 判定を
    // 通りうる。契約 §15 の spawn は生存中の同 surface_id に InvalidState を返すので、
    // 直前に is_alive で再確認して冪等性を守る(この経路が無いと 2 本目が上限エラーでない
    // invalid_state になり、UI が「nvim を起動できませんでした」を誤表示する)。
    // 単一スレッドのテストでは decide_editor_spawn と同じスナップショットしか観測できず
    // この再確認だけを単独で踏むテストは書けない(競合そのものが対象のため)。分岐の
    // 構造(spawn 直前・spawn より確実に前にあること)はレビューで担保する。
    if state.pty.is_alive(&sid) {
        return Ok(EditorSpawnPlan::AlreadyLive(sid));
    }

    let session = state.store.get_session(session_id)?;
    let project = state.store.get_project(&session.project_id)?;
    // 契約 §23(M1-3 の所有物)。worktree を作りに行かない(prepare_worktree は使わない)。
    // worktree 未作成のセッションでは repo 直上で開く(設計判断 D2)。
    let cwd = resolve_cwd(&session, &project.repo_path);
    if !cwd.is_dir() {
        return Err(AppError::Io(format!(
            "work tree does not exist: {}",
            cwd.display()
        )));
    }

    // 契約 §18。未検出なら AppError::CliNotFound がそのまま返る(設計書 §12 のガイド付きエラー)。
    // 裸の "nvim" ではなく解決済みの絶対パスを渡す理由は D5 / 追加提案 3 を参照。
    let program = resolve("nvim")?;
    // 契約 §18。OnceLock キャッシュなので 2 回目以降は探査コスト 0。
    let env = build_editor_env(launch);

    Ok(EditorSpawnPlan::Spawn(SpawnSpec {
        surface_id: sid,
        program: program.display().to_string(),
        args: Vec::new(),
        cwd,
        env,
        // フロントが attach 直後に fitTerminal の実寸で resize_pty を呼ぶ。
        // nvim は SIGWINCH で再レイアウトするので初期値は暫定でよい。
        cols: 80,
        rows: 24,
    }))
}

/// `plan_editor_spawn_with` の実環境版。`session/mod.rs:199` の `plan_agent_spawn` と
/// 同型の薄いラッパで、契約 §18 の公開 API(`probe_login_env` / `resolve_program`)へ
/// 委譲する。
fn plan_editor_spawn(state: &AppState, session_id: &str) -> AppResult<EditorSpawnPlan> {
    plan_editor_spawn_with(state, session_id, probe_login_env(), resolve_program)
}

/// nvim 用 PTY を遅延起動する。既に live なら何もせず同じ surface_id を返す(冪等)。
#[tauri::command]
pub async fn spawn_editor(
    state: State<'_, AppState>,
    app: AppHandle,
    session_id: String,
) -> AppResult<String> {
    match plan_editor_spawn(&state, &session_id)? {
        EditorSpawnPlan::AlreadyLive(sid) => Ok(sid),
        EditorSpawnPlan::Spawn(spec) => {
            let sid = spec.surface_id.clone();
            state.pty.spawn(&app, spec)?;
            // ここで pty.spawn() の直後に返ることに、フロント側の live 書き込みの
            // 正しさが依存している。pty://exit は、いま起こした子プロセスが実際に
            // 終了した後にしか emit されないため、このコマンド応答のエンキューより
            // 厳密に後になる。ここへ待ちを伴う処理を足すと、pty://exit がコマンド
            // 応答を追い越し、exited を live が上書きしうる —— UI は live だが
            // PTY は死んでいる状態になり、画面が黒く再起動ボタンも出ない。
            // 足す場合はフロント側(EditorSurface.tsx)に epoch ガード(live を
            // 書くのは現在が spawning のときだけ)を入れること。
            Ok(sid)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CliKind, Session};
    use crate::pty::launch_env::LaunchEnv;
    use crate::pty::{PtySink, DEFAULT_COLS, DEFAULT_ROWS};
    use crate::state::test_support::app_state;
    use crate::store::test_support::{insert_test_session, open_temp};
    use std::sync::Arc;

    fn env_of(env: &[(String, String)], key: &str) -> Option<String> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    /// 契約 §18 の probe_login_env() が返す値の代わり。純粋関数なので実探査は不要。
    fn launch(path: &str, lang: &str) -> LaunchEnv {
        LaunchEnv {
            path: path.to_string(),
            lang: lang.to_string(),
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // ---- build_editor_env(設計判断 D6 / D11) ----

    #[test]
    fn env_passes_through_probed_path_and_never_declares_terminal_type() {
        let env = build_editor_env(&launch("/opt/homebrew/bin:/usr/bin", "ja_JP.UTF-8"));
        assert_eq!(
            env_of(&env, "PATH").as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        // 契約 §60.6: TERM / COLORTERM の所有は PtySurface::spawn。ここには現れない。
        // (元は「xterm-256color / truecolor が入る」ことを見る 2 assert だった。§60.8 で反転した)
        assert_eq!(env_of(&env, "TERM"), None);
        assert_eq!(env_of(&env, "COLORTERM"), None);
    }

    #[test]
    fn env_never_injects_kamux_session_id() {
        // エディタは hooks に関与しない(設計判断 D11)
        let env = build_editor_env(&launch("/usr/bin", "ja_JP.UTF-8"));
        assert_eq!(env_of(&env, "KAMUX_SESSION_ID"), None);
    }

    #[test]
    fn keeps_utf8_locale_from_login_shell() {
        assert_eq!(
            env_of(
                &build_editor_env(&launch("/usr/bin", "ja_JP.UTF-8")),
                "LANG"
            )
            .as_deref(),
            Some("ja_JP.UTF-8")
        );
        assert_eq!(
            env_of(&build_editor_env(&launch("/usr/bin", "en_US.utf8")), "LANG").as_deref(),
            Some("en_US.utf8")
        );
    }

    #[test]
    fn falls_back_when_the_locale_is_not_utf8() {
        // 契約 §18 は lang が非空であることを保証するが、ログインシェルが
        // LANG=C を明示している環境ではその値がそのまま来る。C ロケールの
        // nvim は日本語を化けさせるので、ここで UTF-8 に落とす
        assert_eq!(
            env_of(&build_editor_env(&launch("/usr/bin", "C")), "LANG").as_deref(),
            Some("en_US.UTF-8")
        );
        assert_eq!(
            env_of(&build_editor_env(&launch("/usr/bin", "POSIX")), "LANG").as_deref(),
            Some("en_US.UTF-8")
        );
        // §18 が壊れて空文字が来ても化けさせない
        assert_eq!(
            env_of(&build_editor_env(&launch("/usr/bin", "")), "LANG").as_deref(),
            Some("en_US.UTF-8")
        );
    }

    // ---- spawn 判定(設計判断 D3) ----

    #[test]
    fn only_editor_suffixed_surfaces_count() {
        assert!(is_editor_surface("abc:editor"));
        assert!(!is_editor_surface("abc:agent"));
        assert!(!is_editor_surface("editor"));
        // Task 3 レビュー Minor #4: ":editor" を「末尾」ではなく「部分文字列」として
        // 含むだけの入力("abc:editorial" は ":editor" を substring に持つ)。
        // `ends_with` → `contains` の変異はこの入力が無いと生き残る。
        assert!(!is_editor_surface("abc:editorial"));
        let live = ids(&["a:agent", "a:editor", "b:agent", "b:editor", "c:agent"]);
        assert_eq!(count_live_editor_surfaces(&live), 2);
    }

    #[test]
    fn already_live_target_is_idempotent() {
        let live = ids(&["a:editor", "b:editor", "c:editor"]);
        assert_eq!(
            decide_editor_spawn("b:editor", &live, MAX_LIVE_EDITOR_SURFACES),
            EditorSpawnDecision::AlreadyLive
        );
    }

    #[test]
    fn spawns_while_under_the_limit() {
        let live = ids(&["a:editor", "b:editor"]);
        assert_eq!(
            decide_editor_spawn("c:editor", &live, 3),
            EditorSpawnDecision::Spawn
        );
    }

    #[test]
    fn refuses_at_the_limit() {
        let live = ids(&["a:editor", "b:editor", "c:editor"]);
        assert_eq!(
            decide_editor_spawn("d:editor", &live, 3),
            EditorSpawnDecision::LimitReached
        );
    }

    #[test]
    fn agent_surfaces_do_not_consume_the_editor_budget() {
        let live = ids(&["a:agent", "b:agent", "c:agent", "d:agent", "e:agent"]);
        assert_eq!(
            decide_editor_spawn("a:editor", &live, 3),
            EditorSpawnDecision::Spawn
        );
    }

    #[test]
    fn decision_is_only_the_first_gate() {
        // live のスナップショットは判定時点のもの。同一セッションへの並行 spawn_editor は
        // 両方が Spawn を得うるので、呼び出し側は spawn 直前に is_alive で再確認する
        // (Task 4 参照。契約 §15 の spawn は生存中の同 surface_id に InvalidState を返す)
        let live: Vec<String> = Vec::new();
        assert_eq!(
            decide_editor_spawn("a:editor", &live, 3),
            EditorSpawnDecision::Spawn
        );
        assert_eq!(
            decide_editor_spawn("a:editor", &live, 3),
            EditorSpawnDecision::Spawn
        );
    }

    // cwd 算出のテストはここには置かない。契約 §23 のとおり `resolve_cwd` は
    // `session/cli_args.rs`(M1-3 の所有物)にあり、そちらでテスト済みのため。

    // ---- plan_editor_spawn_with(Task 4: spawn_editor の AppHandle 非依存部分) ----

    /// 実 PTY を起こさずに登録だけ済ませる no-op sink。`on_data` / `on_exit` は捨てる。
    struct NoopSink;

    impl PtySink for NoopSink {
        fn on_data(&self, _surface_id: &str, _base64: String, _seq: u64) {}
        fn on_exit(&self, _surface_id: &str, _exit_code: Option<i32>) {}
    }

    fn dummy_spawn_spec(surface_id: &str) -> SpawnSpec {
        SpawnSpec {
            surface_id: surface_id.to_string(),
            program: "/bin/cat".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            env: Vec::new(),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
        }
    }

    /// テスト用に実 session/project を 1 件挿入した `AppState` を返す。
    /// `repo_path` は呼び出し側が既存/不在いずれのパスも渡せる
    /// (存在検証は `plan_editor_spawn_with` 側の仕事なので、ここでは検証しない)。
    ///
    /// `worktree` が `Some((branch, worktree_path))` なら `store.set_worktree` を当てて
    /// `resolve_cwd`(契約 §23)の `Some` 分岐(worktree モード)を、`None` なら
    /// `repo_path` を使う分岐(in_place)を踏む。I-1 レビュー指摘: 新規 4 テストの
    /// フィクスチャが `worktree_path: None` しか作っていなかったため、
    /// `resolve_cwd(&session, &project.repo_path)` を `PathBuf::from(&project.repo_path)`
    /// に変異させても全緑のまま通っていた。この 1 つの関数の型に両方の分岐を持たせ、
    /// 呼び出し側が明示的に選ぶ形にする。
    fn app_state_with_session(
        repo_path: &str,
        worktree: Option<(&str, &str)>,
    ) -> (tempfile::TempDir, AppState, Session) {
        let (dir, store) = open_temp();
        let project_id = store
            .insert_project("kamux", repo_path, CliKind::Claude)
            .expect("insert_project")
            .id;
        let session = insert_test_session(&store, &project_id, "edit target");
        if let Some((branch, worktree_path)) = worktree {
            store
                .set_worktree(&session.id, branch, worktree_path)
                .expect("set_worktree");
        }
        (dir, app_state(store), session)
    }

    #[test]
    fn plan_editor_spawn_with_builds_a_spawn_spec_from_the_resolved_program_and_repo_cwd() {
        let repo = tempfile::tempdir().expect("tempdir");
        let (_dir, state, session) =
            app_state_with_session(repo.path().to_str().expect("utf8"), None);

        let plan = plan_editor_spawn_with(
            &state,
            &session.id,
            &launch("/opt/homebrew/bin:/usr/bin", "ja_JP.UTF-8"),
            |program| {
                assert_eq!(program, "nvim", "must resolve the nvim binary, not a shell");
                Ok(PathBuf::from("/fake/bin/nvim"))
            },
        )
        .expect("plan");

        let spec = match plan {
            EditorSpawnPlan::Spawn(spec) => spec,
            EditorSpawnPlan::AlreadyLive(sid) => panic!("expected Spawn, got AlreadyLive({sid})"),
        };
        assert_eq!(
            spec.surface_id,
            surface_id(&session.id, SurfaceKind::Editor)
        );
        assert_eq!(spec.program, "/fake/bin/nvim");
        assert!(spec.args.is_empty());
        assert_eq!(spec.cwd, repo.path());
        assert_eq!(spec.cols, 80);
        assert_eq!(spec.rows, 24);
        assert_eq!(
            env_of(&spec.env, "PATH").as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        assert_eq!(env_of(&spec.env, "LANG").as_deref(), Some("ja_JP.UTF-8"));
    }

    /// I-1: `_and_repo_cwd`(上のテスト)と対になる worktree 側。契約 §23 の
    /// `resolve_cwd` は `worktree_path` が `Some` ならそちらを、`None` なら
    /// `repo_path` を返す(00-contracts.md:6518: 取り違えると「エディタが worktree
    /// ではなくリポジトリ直上で開く」障害になり、ユーザーは気づけない)。
    /// リポジトリ側の `repo_path` はあえて worktree とは別の(存在しない)場所にし、
    /// `resolve_cwd` を `PathBuf::from(&project.repo_path)` に取り違えたら
    /// `cwd.is_dir()` チェックで Err になって検出できるようにする。
    #[test]
    fn plan_editor_spawn_with_builds_a_spawn_spec_from_the_resolved_program_and_worktree_cwd() {
        let repo_root = tempfile::tempdir().expect("tempdir");
        let non_worktree_repo_path = repo_root.path().join("repo-path-must-not-be-used");
        let worktree = tempfile::tempdir().expect("tempdir");
        let (_dir, state, session) = app_state_with_session(
            non_worktree_repo_path.to_str().expect("utf8"),
            Some((
                "session/fix-editor-cwd",
                worktree.path().to_str().expect("utf8"),
            )),
        );

        let plan = plan_editor_spawn_with(
            &state,
            &session.id,
            &launch("/usr/bin", "ja_JP.UTF-8"),
            |_| Ok(PathBuf::from("/fake/bin/nvim")),
        )
        .expect("plan");

        let spec = match plan {
            EditorSpawnPlan::Spawn(spec) => spec,
            EditorSpawnPlan::AlreadyLive(sid) => panic!("expected Spawn, got AlreadyLive({sid})"),
        };
        assert_eq!(spec.cwd, worktree.path());
    }

    /// I-2: 契約 §18(M3-1 は独自のガイド文を組み立てない)/ §19(nvim 未検出は
    /// `CliNotFound`)。`resolve("nvim")?` の `Err` を `AppError::InvalidState` へ
    /// 包み直すと、上限超過(同じ `InvalidState` 型。editor.rs 107 行目)と
    /// 見分けが付かなくなる(118-119 行目のコメントが警戒している症状そのもの)。
    /// `session/mod.rs:637`
    /// (`plan_agent_spawn_does_not_create_a_worktree_when_the_binary_cannot_be_resolved`)
    /// と同型の透過確認。
    #[test]
    fn plan_editor_spawn_with_lets_cli_not_found_pass_through_unwrapped() {
        let repo = tempfile::tempdir().expect("tempdir");
        let (_dir, state, session) =
            app_state_with_session(repo.path().to_str().expect("utf8"), None);

        let failing_resolve =
            |_: &str| -> AppResult<PathBuf> { Err(AppError::CliNotFound("nvim".to_string())) };
        let err = plan_editor_spawn_with(
            &state,
            &session.id,
            &launch("/usr/bin", "C"),
            failing_resolve,
        )
        .expect_err("must fail when nvim cannot be resolved");

        assert!(matches!(err, AppError::CliNotFound(_)), "actual: {err:?}");
    }

    #[test]
    fn plan_editor_spawn_with_rejects_when_the_work_tree_directory_is_missing() {
        let repo = tempfile::tempdir().expect("tempdir");
        let missing = repo.path().join("does-not-exist");
        let (_dir, state, session) = app_state_with_session(missing.to_str().expect("utf8"), None);

        let err = plan_editor_spawn_with(&state, &session.id, &launch("/usr/bin", "C"), |_| {
            panic!("must not resolve nvim before the cwd check")
        })
        .expect_err("missing work tree must be rejected");

        match err {
            AppError::Io(msg) => {
                assert!(
                    msg.contains(missing.to_str().expect("utf8")),
                    "must name the missing directory: {msg}"
                );
                assert!(msg.contains("work tree does not exist"), "{msg}");
            }
            other => panic!("expected AppError::Io, got {other:?}"),
        }
    }

    #[test]
    fn plan_editor_spawn_with_is_idempotent_when_the_surface_is_already_live() {
        // session/project を 1 件も挿入しない空の store を使う。この後 get_session が
        // 呼ばれれば NotFound になるはずなので、Ok(AlreadyLive) が返ること自体が
        // 「live チェックの時点で DB へ触れずに帰った」ことの証拠になる。
        let (_dir, store) = open_temp();
        let state = app_state(store);
        let sid = surface_id("sess-without-a-row", SurfaceKind::Editor);
        state
            .pty
            .spawn_with_sink(Arc::new(NoopSink), dummy_spawn_spec(&sid))
            .expect("spawn the dummy editor surface");

        let plan = plan_editor_spawn_with(
            &state,
            "sess-without-a-row",
            &launch("/usr/bin", "C"),
            |_| panic!("must not resolve nvim when already live"),
        )
        .expect("plan");

        match plan {
            EditorSpawnPlan::AlreadyLive(got) => assert_eq!(got, sid),
            EditorSpawnPlan::Spawn(spec) => panic!("expected AlreadyLive, got Spawn({spec:?})"),
        }
    }

    #[test]
    fn plan_editor_spawn_with_refuses_once_the_editor_surface_limit_is_reached() {
        let (_dir, store) = open_temp();
        let state = app_state(store);
        for id in ["a", "b", "c"] {
            let sid = surface_id(id, SurfaceKind::Editor);
            state
                .pty
                .spawn_with_sink(Arc::new(NoopSink), dummy_spawn_spec(&sid))
                .expect("spawn a dummy editor surface");
        }

        let err = plan_editor_spawn_with(&state, "d", &launch("/usr/bin", "C"), |_| {
            panic!("must not resolve nvim once the limit is reached")
        })
        .expect_err("the 4th editor surface must be refused");

        match err {
            AppError::InvalidState(msg) => {
                assert!(
                    msg.contains(&MAX_LIVE_EDITOR_SURFACES.to_string()),
                    "must name the limit: {msg}"
                );
                assert!(
                    msg.contains(":qa"),
                    "must tell the user how to free a slot: {msg}"
                );
            }
            other => panic!("expected AppError::InvalidState, got {other:?}"),
        }
    }
}
