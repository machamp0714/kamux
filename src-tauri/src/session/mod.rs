pub mod cli_args;
pub mod heuristics;
pub mod resume_tracker;
pub mod runtime_state;
pub mod workspace;

use std::path::PathBuf;

use tauri::{AppHandle, State};

use crate::error::{AppError, AppResult};
use crate::model::{CliKind, KanbanStatus, Session, SessionMode, SessionPatch, SurfaceKind};
use crate::pty::launch_env::{probe_login_env, resolve_program, LaunchEnv};
use crate::pty::{surface_id, SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};
use crate::session::heuristics::sink_impl::{attach_heuristics, detach_heuristics};
use crate::session::heuristics::OutputObserver;
use crate::session::runtime_state::StateInput;
use crate::state::AppState;
use crate::store::now_ms;
use cli_args::{
    apply_hooks, binary_name, build_launch_command, login_shell, resume_mode, resume_plan,
    ResumeMode,
};
use workspace::{apply_start_kanban_transition, prepare_worktree};

/// 起動が「新規」か「再開」かの判別子（M2-4）。
///
/// **`ResumeMode` そのものを引数にできない。** `ResumeMode<'a>` は
/// `SessionId(&'a str)` で借りるが、その借用元の `Session` は
/// `plan_agent_spawn_with` の**中**で `state.store.get_session(id)` される
/// （契約 §123.6 のハザード）。判別子だけを渡し、`ResumeMode` は関数の中で
/// `resume_plan()` から導く。
///
/// `Fresh` は `ResumePlan::FreshStart` とは別物である —— こちらは「そもそも
/// 会話復元を試みない起動」であり、`claude_session_id` の有無を見ない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnIntent {
    /// `start_session`。会話は復元しない。
    Fresh,
    /// `resume_session`（M2-4）。復元方法は `resume_plan()` が決める。
    Resume,
}

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
    intent: SpawnIntent,
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

    // ここから下が「起動フェーズ」= これから PTY を上げようとして上げられなかった区間
    // である。**この即時実行クロージャの中で起きた `Err` だけが ❌ になる**
    // （契約 §40.3 の許可リスト。`start_session` の列挙は §63.5 が 5 段に更新した:
    // `resolve_program` / `prepare_worktree` / `set_worktree` / `build_launch_command` /
    // `PtyManager::spawn`。5 段目だけはこの関数の外（`start_session`）にある）。
    //
    // **上の二重起動ガード（`InvalidState`）と `get_session` / `get_project` を
    // ここへ入れてはいけない。** 判定基準は「その `Err` の時点でセッションが起動して
    // いないことが確実か」であり（契約 §40.3。§63.5 でも 1 文字も変わっていない）、
    // 事前条件エラーは「すでに上がっている」ことの表明である。生きているセッションに
    // ❌ を書くと、`error` 行は `Spawned` しか受け付けず、ガードは PTY が生きている
    // 限り `InvalidState` を返し続けるので `Spawned` は永遠に来ない —— カードが ❌ の
    // まま永久に固着する。
    //
    // cwd の `is_dir()` 検証（下）は §40.3 が名前で列挙した 5 段のどれでもないが、
    // `set_worktree` と `build_launch_command` の**間**にある起動フェーズの一部であり、
    // spawn より前なのでセッションは確実に起動していない。§40.3 の判定基準に照らして
    // クロージャの中に置く（そもそも portable-pty が cwd を黙って $HOME へ倒すのを
    // 防ぐための spawn の前提条件チェックであり、5 段目の一部と読むのが素直である）。
    let planned = (|| -> AppResult<SpawnSpec> {
        // 実行ファイルを解決する（契約 §18。claude/codex 未検出はここで CliNotFound）。
        // `binary_name` は Claude / Codex だけ `Some` を返す。Shell と Custom は**両方**
        // `None` を返す —— Custom は `cli_args::build_launch_command` の Custom の腕が
        // `$SHELL -l -c "<cli_command>"` を組む（シェル経由起動）ため、Shell と同じ
        // ログインシェルへ意図的に相乗りさせている。`None` を「シェルだから」と早合点して
        // Custom を別経路に倒すと、Custom セッションが誤ったプログラムへ飛ぶ
        // （Task 7 レビューが名指しで警告した事故）。
        //
        // **`prepare_worktree`（下）より前に解決すること。** 根拠は「`claude` 未検出という
        // 最も日常的な失敗で、worktree という副作用（git ブランチ作成・ディレクトリ作成）を
        // 作らずに済むこと」である（契約 §63.4 の 1 段目の理由）。ここが `prepare_worktree`
        // より後ろだと、claude 未インストール環境で「worktree はディスク上に作られたが
        // CliNotFound で spawn まで届かない」状態になる。
        //
        // `branch` は NULL でありうる（契約 §62 案 D）。ユーザーがブランチ欄を編集していなければ
        // `sessionForm.ts` は `create_session` へ `branch: null` を送る —— `proposeBranchName`
        // の出力は入力欄の表示にのみ使われ、DB へは焼かれない。`prepare_worktree` は
        // `session.branch == None` のとき `suggest_branch_name` で空いている名前を確定する。
        //
        // `set_worktree`（下）は `prepare_worktree` の直後、spawn より前で呼ばれる
        // （契約 §63.1 / §63.4）。設計判断 6（「spawn が `Ok` を返した後にのみ DB を更新する」）
        // の適用範囲は契約 §63.1 により `kanban_status` / `sort_order` に限定されている
        // —— `branch` / `worktree_path` はこの解決の直後、spawn 成功より前に永続化される。
        //
        // `start_session` の起動フェーズの順序（契約 §63.4。順序の根拠はコード上ここにしか
        // 無い —— 契約 §63.6 は `prepare_worktree` に `&Store` を持たせるチョークポイント化を
        // 却下し、代わりに置くものとしてこの順序規則そのものを選んだ）:
        //   1. resolve_program（副作用が無く cwd にも依存しないので最初に置く）
        //   2. prepare_worktree
        //   3. Store::set_worktree
        //   4. build_launch_command
        //   5. PtyManager::spawn
        let program = match binary_name(session.cli_kind) {
            Some(name) => resolve(name)?.display().to_string(),
            None => login_shell(),
        };

        // worktree モードなら必要に応じて worktree を作る/再利用し、in_place ならそのまま
        // repo_path を返す（契約 §13 / 設計判断 8）。`session.branch` / `session.worktree_path`
        // をここで in-memory に確定させる。
        let cwd = prepare_worktree(&project, &mut session)?;

        // 契約 §63.1 / §63.4: `branch` / `worktree_path` の永続化は `prepare_worktree` が
        // `Ok` を返した直後、これ以降のどの失敗しうる処理（cwd の `is_dir()` 検証・
        // `build_launch_command`・`PtyManager::spawn`）よりも前に行う。
        //
        // 設計判断 6（「spawn が `Ok` を返した後にのみ DB を更新する」）は
        // `kanban_status` / `sort_order` にのみ適用される —— 判断 6 が守ろうとしたのは
        // 「カードが In Progress へ飛ぶのは DB が嘘をつくことに等しい」であり、対象は
        // 列移動だけである。`branch` / `worktree_path` は性質が逆で、ディスク上に実在する
        // worktree を DB に記録するのだから、書けば DB は真実に近づき、書かなければ嘘を
        // つく。ここで書かずに `build_launch_command` や `spawn` が失敗すると、worktree と
        // git ブランチはディスク上に残ったまま DB には記録されず、`resolve_cwd`（M3-1 の
        // エディタ）がリポジトリ直上を開く・resume（M2-4）が `InvalidState` で弾かれる・
        // 判断 8 の再利用腕が同じ branch で `create_worktree` を再試行して
        // 「branch already exists」により恒久的に起動不能になる、という害が生じる
        // （契約 §63 が実測した 5 つの消費者のうち複数）。
        //
        // `Store::update_session` の SET 句は title/description/kanban_status/sort_order/
        // archived_at/updated_at だけで branch/worktree_path を含まない
        // （`session_dao.rs` 実測）。`set_worktree` を別途呼ばない限りここで確定させた値は
        // DB に残らず、次回起動でまた新規 worktree を作ってしまう。worktree モードなら
        // 常に両方 Some、in_place なら常に両方 None なので `if let` の両側同時成立だけを
        // 見ればよい。失敗（`AppError::NotFound` 等）は `?` で中断し、セッションを
        // 起動しない —— DB に書けなかった worktree で起動を続けると、この関数が
        // 埋めようとした真実性の穴がそのまま残る。
        if let (Some(branch), Some(worktree_path)) = (&session.branch, &session.worktree_path) {
            state
                .store
                .set_worktree(&session.id, branch, worktree_path)?;
        }

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

        // 再開の決定（M2-4）。`ResumeMode` は必ず `resume_plan()` から導き、ここで
        // `cli_kind` を直接見て組み立てない（契約 §123.6 の 4）—— codex / shell /
        // custom は `resume_plan()` が必ず `FreshStart` を返すので、非 `None` の
        // `ResumeMode` が `build_launch_command` の
        // `CliKind::Claude | CliKind::Codex` の腕へ届く経路が構造上できない。
        //
        // `ResumePlan` を先に `let` で束縛するのは、`ResumeMode<'a>` が
        // `ResumePlan` の中の `String` を借りるためである（契約 §123.6 のハザード）。
        // 一時値のままだと文の終わりで drop される。
        let plan = match intent {
            SpawnIntent::Fresh => None,
            SpawnIntent::Resume => Some(resume_plan(&session)),
        };
        let resume = match &plan {
            Some(plan) => resume_mode(plan),
            None => ResumeMode::None,
        };

        // 起動コマンドを組み立てる（契約 §23 の純粋関数）。
        let launch = build_launch_command(&session, &program, &cwd, launch_env, resume)?;

        // hooks 由来の値を重ねる。argv（`--settings`）は claude 限定、env
        // （`KAMUX_HOOKS_SOCK`）は全 cli_kind 共通である（契約 §30.2。分界の理由は
        // `apply_hooks` の doc）。`build_launch_command` 自身のシグネチャには手を
        // 入れない（契約 §23 / §30.2.1）。`state.hooks` は hooks が無効なら None の
        // ままで、`apply_hooks` はそのとき何も足さない。
        let launch = apply_hooks(&session, launch, state.hooks.as_ref());

        Ok(SpawnSpec {
            surface_id: sid,
            program: launch.program.to_string_lossy().into_owned(),
            // KAMUX_SESSION_ID / PATH / LANG は build_launch_command が、
            // KAMUX_HOOKS_SOCK（全 cli_kind 共通。契約 §30.2）と --settings
            // （claude 限定）は apply_hooks が入れている。
            // ここでは push しない（契約 §23「呼び出し側は一切 push しない」/ §31.4）。
            env: launch.env,
            args: launch.args,
            cwd: launch.cwd,
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
        })
    })();

    let spec = match planned {
        Ok(spec) => spec,
        Err(err) => {
            // 契約 §40.3 / §63.5: 起動フェーズの Err を ❌ にする唯一の場所。
            // 生 stderr（`AppError` の `Display`）を加工せずそのまま残す（契約 §2 / §6）。
            // トーストは数秒で消えるがカードには痕跡が残る —— それが `error` の存在理由。
            state.runtime.sender().mark_error(id, &err.to_string());
            return Err(err);
        }
    };

    Ok((session, spec))
}

/// `plan_agent_spawn_with` の実環境版。契約 §18 の公開 API（`probe_login_env` /
/// `resolve_program`）へ委譲する薄いラッパ（契約 §60.4）。
///
/// `intent` をここにも通してあるのは、agent サーフェスの起動が実環境の PATH 解決
/// （`resolve_program`）とログイン env 探査（`probe_login_env`）へ入る口が
/// この関数だからである（契約 §123.6 の 2。editor サーフェスは
/// `pty::editor::plan_editor_spawn` が別に持つ）。ここへ通さないと M2-4 の
/// `resume_session` は §18 の 2 つを自前で呼ぶことになり、resume 経路だけが
/// 別の解決手順を持つ。`resume_session` はここへ `SpawnIntent::Resume` を渡す。
fn plan_agent_spawn(
    state: &AppState,
    id: &str,
    intent: SpawnIntent,
) -> AppResult<(Session, SpawnSpec)> {
    // `intent` の素通し（この 1 行の `intent` の選択）はどのテストからも観測
    // されていない。取り違え（例: 引数を `SpawnIntent::Fresh` へ潰す）を打つ
    // 変異は全緑になることを実測済み（レビュー I-2 / M-A、HEAD 424d8a2）。理由:
    // `plan_agent_spawn` は `probe_login_env()` / `resolve_program`（契約 §18）
    // という実環境（`$SHELL -ilc` の実行・実 PATH 探索）に触れる seam の外側の
    // ラッパであり（契約 §60.4）、`resolve_program` へ本物の `claude` を注入
    // しない限り `SpawnIntent::Resume` の分岐へテストから到達できない
    // （契約 §14「実 `claude` は使わない」の下では構成できていない）。
    // したがって、この 1 トークンの取り違えは `start_session`（`mod.rs` 内の
    // 既存の M-1 コメントを参照）と同じ扱いで目視レビューにより担保する。
    plan_agent_spawn_with(state, id, probe_login_env(), resolve_program, intent)
}

/// PTY spawn が成功した**後**にのみ呼ぶ。カンバン列の遷移判定・永続化を行う
/// （設計判断 6: 失敗時にカードが In Progress へ飛ぶと DB が嘘をつく）。
///
/// `branch` / `worktree_path` の永続化はここでは**行わない**。契約 §63.1 / §63.4 に
/// より、それは `plan_agent_spawn_with` 側（`prepare_worktree` の直後、spawn より前）
/// で既に完了している —— 判断 6 が「spawn 成功後にのみ書く」を要求するのは
/// `kanban_status` / `sort_order` だけであり、`branch` / `worktree_path` は逆の性質
/// （書けば DB が真実に近づく）を持つため、早期に確定させる。
///
/// `AppHandle` も `PtyManager` も要らないため、実 PTY を起動せずに固定できる。
/// `start_session` に残る `AppHandle` 依存の行は `state.pty.spawn(&app, spec)` の
/// 1 行だけになる。
fn commit_started_session(state: &AppState, session: &mut Session) -> AppResult<Session> {
    if session.kanban_status != KanbanStatus::Backlog {
        // 設計書 §5.3: 自動遷移は backlog -> in_progress のみ。review / done /
        // in_progress のセッションは列を動かさない。ここで `update_session` を
        // 呼ぶと、変更する値が無いのに `updated_at` だけが動く —— §38.2 が禁じる
        // 「ランタイム由来の書き込みで最終更新順を汚す」に当たるため呼ばない。
        // `next_sort_order` すら引かない。
        //
        // 戻り値は `session.clone()` ではなく `get_session` で読み直す。
        // `plan_agent_spawn_with` 側の `set_worktree`（この関数より前、spawn より前に
        // 実行済み）は DB の `updated_at` を自前で進めるが、この関数が受け取る
        // `session`（in-memory）はその呼び出しの結果で更新されていない —— DAO は
        // DB へ書くだけで呼び出し元の構造体は書き換えない。fetch し直さないと
        // 返り値の `updated_at`（および万一のズレがあれば branch/worktree_path）が
        // DB の値より古いまま返ってしまう。
        return state.store.get_session(&session.id);
    }

    let tail = state
        .store
        .next_sort_order(&session.project_id, KanbanStatus::InProgress)?;
    // 純関数。kanban_status == Backlog のときだけ InProgress へ遷移させる（設計判断 6）。
    // last_runtime_state は一切触らない（設計判断 7。M2-1 が pty://exit 購読で一元的に書く）。
    apply_start_kanban_transition(session, tail);

    // 契約 §17: 部分更新は SessionPatch で行う。branch/worktree_path は
    // `plan_agent_spawn_with` 側で set_worktree 済み（契約 §63.4）なので、
    // ここでは kanban_status / sort_order だけを書く。
    state.store.update_session(
        &session.id,
        &SessionPatch {
            kanban_status: Some(session.kanban_status),
            sort_order: Some(session.sort_order),
            ..Default::default()
        },
    )
}

/// ヒューリスティックの装着と PTY spawn をひとまとめにした 1 手（M3-3）。
///
/// `spawn` を引数に出しているのは `plan_agent_spawn_with` と同じ seam の理由による ——
/// `PtyManager::spawn` は `AppHandle`（= `Wry` 固定。契約 §15）を要求し、
/// `tauri::test::mock_builder()` の `MockRuntime` からは到達できない。この関数まで
/// 切り出すと「observer を作って spawn へ渡す」までがユニットテストで固定できる。
///
/// **残る無検査の継ぎ目は `start_session` がここへ渡すクロージャ 1 行だけである。**
/// そこが `spawn`（observer なし）を呼ぶ形に戻っても、ユニットテストからは見えない。
fn spawn_agent_surface_with(
    state: &AppState,
    session: &Session,
    spec: SpawnSpec,
    spawn: impl FnOnce(SpawnSpec, Option<Box<dyn OutputObserver>>) -> AppResult<()>,
) -> AppResult<()> {
    // 契約 §64.3 の 1 行を送る宛先。`spec` はこの後 `spawn` へ move される。
    let surface_id = spec.surface_id.clone();
    let observer = attach_heuristics(&state.heuristics, session);
    spawn(spec, Some(observer)).inspect_err(|_| {
        // spawn に失敗したセッションは読み取りスレッドを持たず、`sink.rs` の `on_exit` も
        // 来ない。ここで外さないとレジストリに死んだ登録が残り続ける。
        //
        // **1 つだけ「生きている登録を外す」形になりうる Err がある** ——
        // `PtyManager` の排他が返す `InvalidState("surface already running")` である。
        // `start_session` からは到達しない（`plan_agent_spawn_with` の二重起動ガードが
        // 副作用の前に同じ条件で弾く）が、その `is_alive` チェックと spawn の間には
        // 窓があり、同じセッションへの `start_session` が並行すると理論上は踏める。
        // そのとき先行して走った `register` が既に生きているエントリを押し出している
        // ので、**外さないほうが状態が壊れる**（押し出された側は `register` の resume
        // 腕で既に停止済みで、レジストリには新しい死んだエントリだけが残る）。
        detach_heuristics(&state.heuristics, &session.id);
    })?;

    // 契約 §64.3: **spawn の成功直後**に 1 行だけ送る。フロント（`ptyBridge`）からは
    // 送らない —— ユーザーの最初の入力より前であることを保証できない（§16 の
    // `term.onData` の所有と競合する）。
    //
    // **消費者 3 つ（`start_session` / `resume_session` / `create_scratch_session`）は
    // すべてこの関数を経由するので、ここ 1 箇所に置く。** `spawn_editor`（M3-1）は
    // この関数を通らず `PtyManager::spawn` を直に呼ぶので、構造的に対象外である
    //（§64.3 の「`spawn_editor` は対象外」）。
    //
    // **`Err` を `?` で伝播させない**（契約 §153.3）。ここに来た時点で `spawn` は
    // 成功しており PTY は生きている。伝播させると 3 つの呼び出し側
    //（`start_session` / `resume_session` / `create_and_start_scratch_session_with`）
    // の `mark_error` へ届き、契約 §40.3 の判定基準（「その `Err` の時点で
    // セッションが起動していないことが確実か」）を満たさない `Err` で ❌ を出す。
    // 開くのは「PTY は動いているのにカードが ❌ になり、その間ユーザーは再起動も
    // できない」窓である（`error` から出る唯一の入力は `Spawned` だが、PTY が
    // 生きている間は二重起動ガードが `InvalidState` を返すので `Spawned` が来ない）。
    //
    // **握り潰しではなく縮退である。** 1 行が届かなければ shim は PATH に立たず、
    // 手打ちの `claude` の hook が黙って飛ばなくなる —— その事実は `warn!` に残す
    // （`bootstrap_hooks` が relay / ソケット / settings の 3 つで採っているのと
    // 同じ形。設計書 §12「hooks が使えなくてもアプリは起動する」）。
    // spawn 済みの surface は生きているので、掃除は `sink.rs` の `on_exit` が行う。
    if let Some(line) = crate::shim::shell_path_line(
        session.cli_kind,
        state.hooks.as_ref().and_then(|h| h.shim_dir.as_deref()),
    ) {
        if let Err(err) = state.pty.write(&surface_id, line) {
            tracing::warn!(
                error = %err,
                surface_id = %surface_id,
                "shim PATH line was not delivered to the pty"
            );
        }
    }
    Ok(())
}

/// セッションの agent サーフェスを起動する。
/// サイズは 80x24 で起動し、フロントが attach 直後の fit() → resize_pty で合わせる。
#[tauri::command]
pub async fn start_session(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
) -> AppResult<Session> {
    // 起動フェーズ 1〜4 段（`resolve_program` / `prepare_worktree` / `set_worktree` /
    // `build_launch_command`）の Err に対する `mark_error` は `plan_agent_spawn_with` の
    // 中で済んでいる（契約 §40.3 / §63.5）。事前条件（二重起動ガード）と
    // `get_session` / `get_project` の Err はそこで ❌ の対象から外れている。
    //
    // ここに書く `SpawnIntent::Fresh` の選択もどのテストからも観測されていない
    // —— `SpawnIntent::Resume` へ差し替える変異を打っても全緑になることを
    // 実測済み（レビュー I-2 / M-B、HEAD 424d8a2）。理由: `start_session` の
    // `state.pty.spawn`（後段の `spawn_with_observer` 呼び出し）は Wry 固定
    // （契約 §15）で `MockRuntime` に登録できず、IPC テストからこの関数自体へ
    // 到達できない。したがって、この 1 トークンの取り違え（誤って会話を
    // 復元してしまう）は、直後の 5 段目に既にある M-1 コメントの区間と同じ
    // 扱いで目視レビューにより担保する。
    let (mut session, spec) = plan_agent_spawn(&state, &id, SpawnIntent::Fresh)?;
    // AppHandle に依存する行はこの 1 行だけ（設計判断 6: spawn 成功後にのみ DB を書く）。
    // `state.pty.spawn` が Wry 固定（契約 §15）で MockRuntime に登録できないため、
    // 「spawn 成功後にのみ commit_started_session を呼ぶ」という順序はユニットテストでは
    // 担保できない。この 3 行の構造そのもの（`?` の早期リターンが commit の手前にあること）
    // を目視レビューで担保する（レビュー指摘 M-1）。
    //
    // **起動フェーズの 5 段目**（契約 §63.4 / §63.5）。ここだけは `plan_agent_spawn_with`
    // の外にあるので、`mark_error` もここで呼ぶ。同じ理由（Wry 固定）でユニットテストから
    // 到達できないため、この 4 行も目視レビューで担保する。
    //
    // **M3-3: ヒューリスティックの装着もこの 1 手に含まれる**（`spawn_agent_surface_with`）。
    // observer を渡す先は agent サーフェスに限る（設計 §4.8）。editor サーフェスは
    // `pty::editor` が `spawn`（observer なし）を呼んだままにすることで構造的に外れる。
    if let Err(err) = spawn_agent_surface_with(&state, &session, spec, |spec, observer| {
        state.pty.spawn_with_observer(&app, spec, observer)
    }) {
        state.runtime.sender().mark_error(&id, &err.to_string());
        return Err(err);
    }
    // kanban_status の backlog -> in_progress は M1-4 の責務（`commit_started_session`）。
    // ここでは runtime_state だけを動かす。PTY 終了の検知は `sink.rs` が全 spawn 経路を
    // カバーするので、ここでの登録は不要。
    //
    // **この 1 行を `commit_started_session` へ移さないこと。** あちらは
    // `commit_started_session_never_writes_runtime_state`（判断 7）が
    // 「last_runtime_state を一切書かない」を固定している。
    //
    // 契約 §34.5 の `first_started_at` が記録される唯一の経路でもある
    // （`consume_loop` が `Spawned` を遷移表より前で拾う）。
    state.runtime.sender().send(&id, StateInput::Spawned);
    commit_started_session(&state, &mut session)
}

/// セッションの agent サーフェスを**会話を復元して**起動する（契約 §7 / §75）。
///
/// **`start_session` の双子である。** 独自の起動経路を持たず、同じ
/// `plan_agent_spawn` → `spawn_agent_surface_with` → `send(Spawned)` →
/// `commit_started_session` を通る（契約 §123.3）。違いは 2 つだけ:
///   1. `SpawnIntent::Resume` を渡す（`ResumeMode` は `plan_agent_spawn_with` の
///      中で `resume_plan()` → `resume_mode()` から導かれる）
///   2. spawn の直前に `ResumeTracker::mark_resume_attempt` を呼ぶ（契約 §123.6 の 5）
///
/// **`mark_error` は spawn（起動フェーズ 5 段目）の `Err` に対してだけ呼ぶ。**
/// 1〜4 段の `Err` に対しては `plan_agent_spawn_with` が自分の中で呼んでいるので、
/// `?` で素通しする（契約 §123.6 の 6: 二重に呼ぶと `error` を 2 回書いて
/// イベントが 2 通出る）。
///
/// **`send(Spawned)` は `commit_started_session` より前に置くこと**（`start_session`
/// と同一の順序）。`commit_started_session` は `update_session` の失敗で `?` 早期
/// return しうるので、逆順にすると「PTY は生きているのに `Spawned` が状態機械へ
/// 一度も届かない」——カードは `Idle` のまま、契約 §34.5 の `first_started_at` も
/// 記録されない。
///
/// **この関数自体はユニットテストから到達できない**（`state.pty.spawn_with_observer`
/// が Wry 固定。契約 §15 / §96.4）。`start_session` と同じく、判別子
/// （`SpawnIntent::Resume`）の選択と上記の処理順は目視レビューで担保する ——
/// `SpawnIntent::Fresh` へ差し替える変異が全緑になることを実測した（Task 8 の
/// 変異 M-2。`start_session` 側の同型は M-B が実測済み）。テストで守られているのは
/// この関数が呼ぶ側（`plan_agent_spawn_with` の resume 経路 / `mark_resume_attempt`
/// のガード / `sink.rs` の出し分け）である。
#[tauri::command]
pub async fn resume_session(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
) -> AppResult<Session> {
    let (mut session, spec) = plan_agent_spawn(&state, &id, SpawnIntent::Resume)?;

    // 契約 §123.6 の 5: `mark_resume_attempt` は spawn の直前。
    //
    // `resume_plan()` を `plan_agent_spawn_with` の中と here の 2 回引くが、決定は
    // 必ず一致する —— 判断材料は `cli_kind` / `mode` / `claude_session_id` の 3 つ
    // だけで、間に挟まる `prepare_worktree` はそのどれも書き換えない（`branch` /
    // `worktree_path` のみ）。`ResumeMode<'a>` は `ResumePlan` の中の `String` を
    // 借りるため、決定そのものを関数の外へ持ち出す形は借用が組めない
    // （契約 §123.6 のハザード）。
    //
    // 記録するかどうか（`FreshStart` を弾く）の判断は `ResumeTracker` 側にある ——
    // ここに `if` を書くと、この関数がテストから到達できないためガードを外す変異が
    // 全緑になる（`mark_resume_attempt` の doc を参照）。
    let plan = resume_plan(&session);
    state.resume_tracker.mark_resume_attempt(&id, &plan);

    // 起動フェーズの 5 段目（契約 §63.4 / §63.5）。`plan_agent_spawn_with` の外に
    // あるので `mark_error` もここで呼ぶ。M3-3 のヒューリスティック装着は
    // `spawn_agent_surface_with` に含まれる（契約 §123.3 の理由 3。`spawn`
    // （observer なし）へ戻すと resume 経路だけ沈黙推定が落ちる）。
    if let Err(err) = spawn_agent_surface_with(&state, &session, spec, |spec, observer| {
        state.pty.spawn_with_observer(&app, spec, observer)
    }) {
        // 上で記録した試行を破棄する。**PTY が上がらなかった終了は
        // `PtySink::on_exit` を通らないので `classify_exit` による消費が起きない。**
        // 残すと、次にこのセッションで起きた非ゼロ終了（`start_session` = 会話
        // 復元を試みない起動を含む）が `ResumeFailed` に化ける。何を消すかの
        // 判断は `ResumeTracker` 側にあり（`clear_resume_attempt` の doc）、
        // ここに在るのは呼び出し 1 行だけである —— この関数はユニットテストから
        // 到達できないため（契約 §15 / §96.4）、条件をここに書くと外す変異が
        // 緑になる。
        //
        // ---- 並行 spawn に負けた `Err` について（lesser-evil。**未検証の予測**）----
        //
        // **この段落は測っていない。** 並行 `Err` 経路を再現するテストは無く、
        // この関数自体が到達不能領域である（契約 §96.4）。偽にする変異を作れない
        // 種類の記述なので、観測ではなく構造の読み取りとして残す。根拠は
        // 10 行上の `spawn_agent_surface_with`（`:337` 以降）の逐語コメントと、
        // `ResumeTracker` の `insert` / `remove` の意味差である。
        //
        // 1. **先例が隣に在る。** 同じ `Err` を受ける `spawn_agent_surface_with` の
        //    `inspect_err` は、`PtyManager` の排他が返す
        //    `InvalidState("surface already running")` を名指しし、**それでも
        //    `detach_heuristics` を呼ぶ側に倒している**（押し出された登録は既に
        //    停止済みなので、外さないほうが状態が壊れる）。隣り合う 2 つの後始末が
        //    逆の判断を採ると、次の読み手はどちらが正典か決められない。
        // 2. **ただし非対称がある。隠さない。** heuristics 側は「押し出された死んだ
        //    登録」を外すので損失ゼロだが、**こちらが消すのは勝った側の生きている
        //    試行である。** 後発 B が負けたときに B がここへ来て、勝者 A の
        //    エントリを `remove` する —— 以後 A が非ゼロ終了しても素の `PtyExited`
        //    になり、**「会話は復元されませんでした」が沈黙する（誤沈黙）。**
        //    **並行経路の害は round 2 以前から在った** ——
        //    `mark_resume_attempt` の `insert` 上書きにより、A に `SessionStart` が
        //    届いた後で B が mark すると `session_start_seen` が `false` へ戻り、
        //    **A の成功が誤って `ResumeFailed` になる（誤検知）。** round 2 が
        //    加えたのは誤沈黙の側であって、害を新設したのではない。
        // 3. **頻度が釣り合わない。** 手当ての対象（ふつうの spawn 失敗 =
        //    バイナリ不在・worktree 異常）は日常的に起きる。並行敗北は、同一
        //    セッションへの `resume_session` / `start_session` が並行 IPC で届き、
        //    **UI ゲートと `plan_agent_spawn_with` の二重起動ガードの両方を抜ける**
        //    ことを要する。消す側に倒すのはこの非対称による。
        // 4. **コードで直すなら受け皿は `ResumeTracker` 側である**（例: 世代
        //    トークンを取り「自分が入れたエントリだけ消す」）。**ここに `if` を
        //    書かないこと** —— 到達不能領域なので、条件を外す変異が緑になる。
        state.resume_tracker.clear_resume_attempt(&id);
        state.runtime.sender().mark_error(&id, &err.to_string());
        return Err(err);
    }
    state.runtime.sender().send(&id, StateInput::Spawned);
    commit_started_session(&state, &mut session)
}

/// agent サーフェスを殺す。runtime_state の遷移は M2-1 が担当する（設計判断 7）。
/// `PtyManager::kill` は冪等（契約 §15）なので、事前の `is_alive` 確認は不要。
#[tauri::command]
pub async fn stop_session(state: State<'_, AppState>, id: String) -> AppResult<Session> {
    stop_agent_surface(&state, &id)
}

/// `stop_session` の本体。`AppHandle` を要らないので、そのままユニットテストできる。
fn stop_agent_surface(state: &AppState, id: &str) -> AppResult<Session> {
    let session = state.store.get_session(id)?;
    state
        .pty
        .kill(&surface_id(&session.id, SurfaceKind::Agent))?;
    // **`sink.rs` の `on_exit` 任せにしない。** `kill` は冪等で、既に死んでいる／
    // そもそも登録が無いサーフェスでは `on_exit` が二度と来ないため、
    // そこ任せにするとレジストリに登録が残り続ける。`unregister` は冪等。
    detach_heuristics(&state.heuristics, id);
    // kill は冪等（契約 §15）。既に死んでいても、そもそも登録が無くても Ok が返り、
    // ここで ⛔ が確定する。少し遅れて `sink.rs` から `PtyExited` も届くが、
    // `exited` + `PtyExited` は遷移なしなので DB もイベントも動かない（計画 §6.5）。
    state.runtime.sender().send(id, StateInput::UserStopped);
    Ok(session)
}

/// スクラッチ端末の「作成して DB へ確定させる」部分だけを切り出した構築点
/// (契約 §29.1 / §29.2 / §29.3 / §29.6)。`AppHandle` を要らないのでそのまま
/// ユニットテストできる。
///
/// スクラッチの構築点は `Session::new_backlog` とは別にここへ置く —— `new_backlog`
/// は常に `is_scratch: false` を入れる契約（§29.1 の申し送り）なので、そのまま呼んで
/// フィールドを上書きする。`SessionPatch` は経由しない —— スクラッチかどうかは
/// 作成時に決まり、後から切り替わらない（契約 §29.2）。
///
/// `plan_agent_spawn`（既存の agent サーフェス起動）を経由しない。スクラッチは常に
/// `mode: in_place`（契約 §29.6）だが、`prepare_worktree` の in_place 腕は
/// `project.repo_path` しか返さず、呼び出し元が指定した `cwd` を通す経路が無い。
/// `prepare_worktree` に分岐を足すと §13 の worktree 契約に例外を持ち込むことになり、
/// それは契約 §29.6 が名指しで禁じている。cwd の決定はここで独立に行う。
fn plan_scratch_session_with(
    state: &AppState,
    project_id: &str,
    cwd: Option<String>,
    now: i64,
    launch_env: &LaunchEnv,
) -> AppResult<(Session, SpawnSpec)> {
    let project = state.store.get_project(project_id)?;
    let sort_order = state
        .store
        .next_sort_order(project_id, KanbanStatus::Backlog)?;

    let mut session = Session::new_backlog(
        project_id,
        "Scratch",
        "",
        SessionMode::InPlace,
        None,
        CliKind::Shell,
        None,
        sort_order,
        now,
    );
    // `new_backlog` が入れた既定値 false を、スクラッチだけがここで上書きする
    // (契約 §29.1 / §29.2)。
    session.is_scratch = true;

    let resolved_cwd = match cwd {
        Some(cwd) => PathBuf::from(cwd),
        None => PathBuf::from(&project.repo_path),
    };
    // portable-pty 0.9.0 が cwd の実在を `is_dir()` でしか検証せず、ディレクトリで
    // なければ chdir を試みず黙って $HOME へフォールバックする問題は
    // `plan_agent_spawn_with`（このファイル上方）と同じ。述語を逐語で一致させる。
    if !resolved_cwd.is_dir() {
        return Err(AppError::InvalidState(format!(
            "working directory does not exist or is not a directory: {}",
            resolved_cwd.display()
        )));
    }

    // cli_kind は常に Shell（契約 §29.3）なので binary_name() は常に None ——
    // resolve_program の PATH 探索（契約 §18）を経由しない。ログインシェル自身が
    // 起動対象になる。
    let program = login_shell();
    let launch = build_launch_command(
        &session,
        &program,
        &resolved_cwd,
        launch_env,
        ResumeMode::None,
    )?;
    let launch = apply_hooks(&session, launch, state.hooks.as_ref());

    let spec = SpawnSpec {
        surface_id: surface_id(&session.id, SurfaceKind::Agent),
        program: launch.program.to_string_lossy().into_owned(),
        env: launch.env,
        args: launch.args,
        cwd: launch.cwd,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    };

    // spawn より前に DB へ確定させる。ここが本コマンドの「作成」半分である。
    let session = state.store.insert_session(&session)?;
    Ok((session, spec))
}

/// `create_scratch_session` の `AppHandle` 非依存の本体（契約 §29.3）。
/// `spawn` をクロージャとして受け取るのは `spawn_agent_surface_with` と同じ理由 ——
/// `PtyManager::spawn` は Wry 固定（契約 §15）で `MockRuntime` に登録できず、この形に
/// しない限り「作成 → 永続化 → start」という合成そのものをユニットテストから
/// 固定できない。
///
/// **失敗時はロールバックしない。行は残し、`error` へ遷移させる**
/// （`start_session` / `resume_session` の spawn 失敗と同じ扱い。契約 §40.3）。
/// DB に delete API は無く、削除コマンドの新設は契約 §7.4 / §29.3 の却下記録と
/// 矛盾する。行を残せば `error` の理由がユーザーに見える（契約 §2 の `error` の
/// 存在理由）—— 消すとその痕跡ごと失われる。
fn create_and_start_scratch_session_with(
    state: &AppState,
    project_id: &str,
    cwd: Option<String>,
    now: i64,
    launch_env: &LaunchEnv,
    spawn: impl FnOnce(SpawnSpec, Option<Box<dyn OutputObserver>>) -> AppResult<()>,
) -> AppResult<Session> {
    let (session, spec) = plan_scratch_session_with(state, project_id, cwd, now, launch_env)?;

    if let Err(err) = spawn_agent_surface_with(state, &session, spec, spawn) {
        state
            .runtime
            .sender()
            .mark_error(&session.id, &err.to_string());
        return Err(err);
    }
    // これが「start」半分。作成（上の insert_session）と合わせて 1 コマンドの
    // 原子性を成す（契約 §29.3 の「create_session を分岐させずに別コマンドにした
    // 理由」）。
    state
        .runtime
        .sender()
        .send(&session.id, StateInput::Spawned);
    Ok(session)
}

/// スクラッチ端末（M3-4、契約 §29）。作成と start を 1 コマンドで原子的に行う。
/// cwd が None なら project.repo_path。mode は常に in_place、cli_kind は常に shell
#[tauri::command]
pub async fn create_scratch_session(
    state: State<'_, AppState>,
    app: AppHandle,
    project_id: String,
    cwd: Option<String>,
) -> AppResult<Session> {
    create_and_start_scratch_session_with(
        &state,
        &project_id,
        cwd,
        now_ms(),
        probe_login_env(),
        |spec, observer| state.pty.spawn_with_observer(&app, spec, observer),
    )
}

/// 既存セッション向けのブランチ名提案（契約 §60.1 / §60.1.1）。
///
/// 衝突していれば `-2`, `-3` … を付けた**空いている候補**を返す。
/// ユーザーはこれを編集できる（設計書 §6.5 / 設計判断 3）。
///
/// **新規作成ダイアログからは呼べない** —— その時点で `session_id` が存在しないため。
/// 新規作成時のライブプレビューは TS 側 `proposeBranchName` が担当する（契約 §60.1.1）。
#[tauri::command]
pub async fn suggest_branch_name(
    state: State<'_, AppState>,
    project_id: String,
    title: String,
    session_id: String,
) -> AppResult<String> {
    let project = state.store.get_project(&project_id)?;
    crate::worktree::suggest_branch_name(
        std::path::Path::new(&project.repo_path),
        &title,
        &session_id,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::model::{CliKind, RuntimeState, SessionMode};
    use crate::pty::surface::PtySink;
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
        let state = crate::state::test_support::app_state(store);
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
        let state = crate::state::test_support::app_state(store);
        (dir, state, session, worktree_path)
    }

    fn plan(state: &AppState, id: &str) -> AppResult<(Session, SpawnSpec)> {
        plan_agent_spawn_with(
            state,
            id,
            &fake_launch_env(),
            fake_resolve_program,
            SpawnIntent::Fresh,
        )
    }

    /// `plan` の再開版（M2-4）。`SpawnIntent` 以外の引数は同じものを使う。
    fn plan_resume(state: &AppState, id: &str) -> AppResult<(Session, SpawnSpec)> {
        plan_agent_spawn_with(
            state,
            id,
            &fake_launch_env(),
            fake_resolve_program,
            SpawnIntent::Resume,
        )
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

    /// 分岐表 行 1 の `claude_session_id`。`build_state_with_worktree_session` が
    /// 使う他の値（`session/shell` など）と 1 文字も重ならない値にして、取り違えを
    /// 素通しさせない。
    const CLAUDE_SESSION_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    /// `SpawnIntent::Resume` で呼ぶと、`build_launch_command` へ渡る `ResumeMode` が
    /// `resume_plan()` -> `resume_mode()` の決定になる（分岐表 行 1）。
    ///
    /// `cli_args.rs` の対応表テストは `ResumePlan` -> `ResumeMode` までしか見ておらず、
    /// **その純関数が実際に起動経路から呼ばれていること**はここでしか観測できない。
    /// `spec.args` を見るのはそのため（分岐表に `args` の列を持たせるためではない）。
    #[test]
    fn plan_agent_spawn_with_resume_intent_passes_the_mode_derived_from_the_plan() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Claude, None);
        state
            .store
            .set_claude_session_id(&session.id, CLAUDE_SESSION_ID)
            .expect("set claude_session_id");

        let (_, spec) = plan_resume(&state, &session.id).expect("plan resume");

        assert_eq!(
            spec.args,
            vec!["--resume".to_string(), CLAUDE_SESSION_ID.to_string()]
        );
    }

    /// **§18 の PATH 解決が resume 経路でも効いていること**（契約 §123.3 の理由 1）。
    /// `plan_agent_spawn_resolves_the_binary_via_resolve_program_for_claude` は
    /// `SpawnIntent::Fresh` 側しか見ておらず、resume が独自の起動経路を持てば
    /// （= `resolve_program` を通さずログインシェルや素の "claude" へ倒せば）
    /// あちらは緑のまま素通りする。`ShellEnvGuard` で `$SHELL` を既定値と別の値へ
    /// 差し替えるので、`login_shell()` へ倒す変異と `/fake/bin/claude` は必ず食い違う。
    #[test]
    fn plan_agent_spawn_with_resume_intent_resolves_the_binary_to_an_absolute_path() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _guard = ShellEnvGuard::set("/tmp/kamux-test-login-shell");
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Claude, None);
        state
            .store
            .set_claude_session_id(&session.id, CLAUDE_SESSION_ID)
            .expect("set claude_session_id");

        let (_, spec) = plan_resume(&state, &session.id).expect("plan resume");

        assert_eq!(spec.program, "/fake/bin/claude");
    }

    /// **§23 の env が resume 経路で落ちていないこと**（契約 §123.3 の理由 2）。
    /// 3 つとも別々の出どころを持つ: `KAMUX_SESSION_ID` は `session.id`、
    /// `PATH` / `LANG` は注入された `launch_env`。`/fake/bin` と `ja_JP.UTF-8` は
    /// 実行環境の実 PATH / 実ロケールと一致しないので、`launch_env` 引数を
    /// 実環境値へ差し替える変異が入っても vacuous にならない。
    #[test]
    fn plan_agent_spawn_with_resume_intent_keeps_the_session_env() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Claude, None);
        state
            .store
            .set_claude_session_id(&session.id, CLAUDE_SESSION_ID)
            .expect("set claude_session_id");

        let (_, spec) = plan_resume(&state, &session.id).expect("plan resume");

        assert!(
            spec.env
                .contains(&("KAMUX_SESSION_ID".to_string(), session.id.clone())),
            "resume の env から KAMUX_SESSION_ID が落ちている: {:?}",
            spec.env
        );
        assert!(
            spec.env
                .iter()
                .any(|(k, v)| k == "PATH" && v == "/fake/bin"),
            "resume の env から PATH が落ちている: {:?}",
            spec.env
        );
        assert!(
            spec.env
                .iter()
                .any(|(k, v)| k == "LANG" && v == "ja_JP.UTF-8"),
            "resume の env から LANG が落ちている: {:?}",
            spec.env
        );
    }

    /// `resume_session` は `resume_plan()` を 2 回引く —— 1 回目は
    /// `plan_agent_spawn_with` の中（`ResumeMode` を導くため）、2 回目は戻り値の
    /// `Session` に対して（`mark_resume_attempt` へ渡すため）。**この 2 つの決定が
    /// 一致することを固定する。**
    ///
    /// レビュー Minor 1 の手当て: 一致の根拠（「`prepare_worktree` は `branch` /
    /// `worktree_path` しか書き換えないので、判断材料の `cli_kind` / `mode` /
    /// `claude_session_id` は動かない」）はコメントに書いてあるだけで、誰も
    /// 観測していなかった。**`prepare_worktree` の周辺が将来 `claude_session_id`
    /// や `mode` を触った瞬間に黙って破れる** —— そのとき `mark_resume_attempt`
    /// は `plan_agent_spawn_with` が実際に使ったのとは違う決定で記録され、
    /// `FreshStart` ガード（裁定 B）の判定が実際の起動と食い違う。
    ///
    /// **恒真の罠を潰す:** 決定が `FreshStart`（= 復元材料が無い側）だと、
    /// 判断材料を落とす変異でも両辺が `FreshStart` のまま一致してしまう。
    /// 復元材料がある `ClaudeResume` で測り、その値そのものも assert する。
    #[test]
    fn plan_agent_spawn_returns_a_session_that_yields_the_same_resume_decision() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Claude, None);
        state
            .store
            .set_claude_session_id(&session.id, CLAUDE_SESSION_ID)
            .expect("set claude_session_id");

        let before = resume_plan(&state.store.get_session(&session.id).expect("get session"));
        assert_eq!(
            before,
            crate::session::cli_args::ResumePlan::ClaudeResume {
                claude_session_id: CLAUDE_SESSION_ID.to_string(),
            },
            "前提: 復元材料が在る決定で測る（FreshStart 同士の一致は恒真）"
        );

        let (planned, _spec) = plan_resume(&state, &session.id).expect("plan resume");

        assert_eq!(
            resume_plan(&planned),
            before,
            "plan_agent_spawn_with の戻り値が再開の決定を変えている\
             （resume_session はこの Session に対して resume_plan を引き直す）"
        );
    }

    /// 契約 §4.6 / §123.6 の 4: codex には非 `None` の `ResumeMode` が渡らない。
    /// `resume_plan()` が codex に対して常に `FreshStart` を返すので、
    /// `build_launch_command` の `CliKind::Claude | CliKind::Codex` の腕（両者で
    /// 共通）に届く `ResumeMode` は `None` だけになる。
    ///
    /// **DB に `claude_session_id` を入れてから測る。** 入れないと、判別が
    /// `cli_kind` ではなく「そもそも復元材料が無い」で通ってしまい、
    /// `resume_plan()` の codex 腕を claude 側へ倒す変異を弁別できない。
    #[test]
    fn plan_agent_spawn_with_resume_intent_never_resumes_codex() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Codex, None);
        state
            .store
            .set_claude_session_id(&session.id, CLAUDE_SESSION_ID)
            .expect("set claude_session_id");
        assert_eq!(
            state
                .store
                .get_session(&session.id)
                .expect("get session")
                .claude_session_id
                .as_deref(),
            Some(CLAUDE_SESSION_ID),
            "前提: 復元材料が DB に在る状態で測る"
        );

        let (_, spec) = plan_resume(&state, &session.id).expect("plan resume");

        assert!(
            spec.args.is_empty(),
            "codex に resume フラグが渡っている: {:?}",
            spec.args
        );
    }

    /// `start_session`（`SpawnIntent::Fresh`）は、DB に `claude_session_id` が
    /// 載っていても会話を復元しない。M1-4 が `ResumeMode::None` 固定で持っていた
    /// 性質を、判別子を足した後も保つ。
    #[test]
    fn plan_agent_spawn_with_fresh_intent_never_resumes() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Claude, None);
        state
            .store
            .set_claude_session_id(&session.id, CLAUDE_SESSION_ID)
            .expect("set claude_session_id");

        let (_, spec) = plan(&state, &session.id).expect("plan spawn");

        assert!(
            spec.args.is_empty(),
            "start_session が会話を復元している: {:?}",
            spec.args
        );
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

    /// `state.hooks` が `Some` のとき、`plan_agent_spawn_with` が組み立てる
    /// `SpawnSpec` に `--settings` と `KAMUX_HOOKS_SOCK` が実際に流れることを固定する
    /// （契約 §31.4 / §102）。`apply_hooks` 単体のテストは `cli_args.rs` にあるが、
    /// `state.hooks.as_ref()` の配線自体（Task 11 の呼び出し側）を固定するのはこのテスト。
    /// **訂正（Task 17 fix round 1 / I-2）**: `state.hooks.as_ref()` の呼び出し側は
    /// もうここだけではない —— `plan_scratch_session_with`（Task 17、shell 経路）が
    /// 2 つ目の呼び出し側を新設した。その鏡像は
    /// `scratch_session::plan_scratch_session_injects_the_hooks_sock_env_even_for_the_shell_cli_kind`
    /// が持つ。
    #[test]
    fn plan_agent_spawn_injects_hooks_settings_and_sock_for_claude() {
        // ENV_LOCK は不要: CliKind::Claude は fake_resolve_program 経由で解決され、
        // login_shell()（$SHELL 読み取り）を踏まない（上の binary_name 分岐と同じ理由）。
        let (_dir, mut state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Claude, None);
        state.hooks = Some(crate::hooks_srv::HooksRuntime {
            socket_path: PathBuf::from("/tmp/kamux-hooks-test.sock"),
            settings_path: PathBuf::from("/tmp/kamux-hooks-test.settings.json"),
            relay_bin: PathBuf::from("/opt/kamux/kamux-relay"),
            shim_dir: None,
        });

        let (_, spec) = plan(&state, &session.id).expect("plan spawn");

        let pos = spec
            .args
            .iter()
            .position(|a| a == "--settings")
            .expect("--settings must be present");
        assert_eq!(spec.args[pos + 1], "/tmp/kamux-hooks-test.settings.json");
        assert!(
            spec.env.contains(&(
                "KAMUX_HOOKS_SOCK".to_string(),
                "/tmp/kamux-hooks-test.sock".to_string()
            )),
            "actual env: {:?}",
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

    /// バイナリ解決（`resolve`）は `prepare_worktree` より**前**に呼ばれなければならない。
    /// 根拠は「`claude` 未検出という最も日常的な失敗で、worktree という副作用
    /// （git ブランチ作成・ディレクトリ作成）を作らずに済むこと」である（契約 §63.4 の
    /// 1 段目の理由）。順序が逆だと、claude 未インストール環境で worktree ディレクトリと
    /// git ブランチだけがディスク上に作られたまま `AppError::CliNotFound` で失敗する。
    ///
    /// `branch` は NULL でありうる（契約 §62 案 D）。ユーザーがブランチ欄を編集していなければ
    /// `sessionForm.ts` は `create_session` へ `branch: null` を送る —— `prepare_worktree`
    /// は `session.branch == None` のとき `suggest_branch_name` で空いている名前を確定する。
    ///
    /// `set_worktree` は `prepare_worktree` の直後、spawn より前で呼ばれる（契約 §63.1 /
    /// §63.4）。設計判断 6 の適用範囲は契約 §63.1 により `kanban_status` / `sort_order` に
    /// 限定されている —— `branch` / `worktree_path` はこの解決の直後、spawn 成功より前に
    /// 永続化される。
    ///
    /// `start_session` の起動フェーズの順序（契約 §63.4。順序の根拠はコード上ここにしか
    /// 無い —— 契約 §63.6 は `prepare_worktree` に `&Store` を持たせるチョークポイント化
    /// を却下し、代わりに置くものとしてこの順序規則そのものを選んだ）:
    ///   1. resolve_program（副作用が無く cwd にも依存しないので最初に置く）
    ///   2. prepare_worktree
    ///   3. Store::set_worktree
    ///   4. build_launch_command
    ///   5. PtyManager::spawn
    ///
    /// `.worktrees/` が作られていないことまで確認することで、順序の入れ替えを弁別する
    /// （`CliNotFound` が返ることだけを見るテストでは、「worktree を作ってから失敗した」
    /// 場合と区別できない）。
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
        let state = crate::state::test_support::app_state(store);

        let failing_resolve =
            |_: &str| -> AppResult<PathBuf> { Err(AppError::CliNotFound("claude".to_string())) };
        let err = plan_agent_spawn_with(
            &state,
            &session.id,
            &fake_launch_env(),
            failing_resolve,
            SpawnIntent::Fresh,
        )
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
        let state = crate::state::test_support::app_state(store);

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
        let state = crate::state::test_support::app_state(store);
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
        let state = crate::state::test_support::app_state(store);
        (dir, state, project)
    }

    /// 契約 §63.1 / §63.4: `branch` / `worktree_path` の永続化は `prepare_worktree` が
    /// 成功した直後、spawn より前に完了していなければならない
    /// （判断 6 は `kanban_status` / `sort_order` にのみ適用され、`branch` /
    /// `worktree_path` はディスク上の worktree を記録するものなので、書けば DB は
    /// 真実に近づく。書かずに `build_launch_command` や `spawn` が失敗すると、
    /// worktree はディスク上に残ったまま DB は永久に `NULL` になり、判断 8 の再利用腕が
    /// 同じ branch で `create_worktree` を再試行して恒久的に起動不能になる）。
    ///
    /// `spawn` 自体は契約 §15 の Wry 固定でユニットテストから到達できないため、
    /// `plan_agent_spawn_with` が `Ok` を返した時点（spawn を呼ぶ前）で DB を読み直し、
    /// `branch` / `worktree_path` が既に埋まっていることを確認する形で
    /// 「spawn 前に永続化されている」ことを固定する。`set_worktree` の呼び出しを
    /// （移動前の）`commit_started_session` 側へ戻す変異を入れると、この時点では
    /// まだ DB に書かれていないため `reloaded.branch` が `None` のままになり赤くなる。
    #[test]
    fn plan_agent_spawn_with_persists_branch_and_worktree_path_before_spawn() {
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
            "Fix login bug",
            "",
            SessionMode::Worktree,
            None,
            CliKind::Shell,
            None,
            1.0,
            now_ms(),
        );
        let session = store.insert_session(&session).expect("insert session");
        let state = crate::state::test_support::app_state(store);

        let (returned, _spec) = plan(&state, &session.id).expect("plan spawn");

        assert!(returned.branch.is_some());
        assert!(returned.worktree_path.is_some());

        // plan_agent_spawn_with はまだ spawn を呼んでいない。この時点で DB を読み直し、
        // branch/worktree_path が既に一致していることが「spawn 前に永続化されている」
        // ことの証拠になる。
        let reloaded = state.store.get_session(&session.id).expect("get_session");
        assert_eq!(
            reloaded.branch, returned.branch,
            "branch must already be persisted before spawn is even attempted"
        );
        assert_eq!(
            reloaded.worktree_path, returned.worktree_path,
            "worktree_path must already be persisted before spawn is even attempted"
        );
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
    /// ではなく、DB を読み直した最新行でなければならない。
    ///
    /// 契約 §63.4 により `set_worktree` は `commit_started_session` の**外**
    /// （`plan_agent_spawn_with` の `prepare_worktree` 直後）で呼ばれるようになった。
    /// `commit_started_session` が受け取る `session`（in-memory）は、その
    /// `plan_agent_spawn_with` の冒頭 `get_session` で読んだときのスナップショットの
    /// ままであり、以後の `set_worktree` や（Review への移動などの）他経路からの
    /// 書き込みで DB の `updated_at` が進んでも、in-memory 側は更新されない
    /// （DAO は DB へ書くだけで呼び出し元の構造体を書き換えない）。ここではその状況を
    /// `stale_session`（`plan_agent_spawn_with` 冒頭の fetch を模した古いコピー）と、
    /// その後に別途走る `set_worktree` / `update_session` で再現する。
    ///
    /// `now_ms()` はミリ秒精度なので、書き込みの間に短い sleep を挟んでミリ秒境界を
    /// 跨がせないと、バグ入りの `clone()` でも偶然 `updated_at` が一致して緑になって
    /// しまう（実測済み: sleep 無しで変異検証したところ検出できなかった）。
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

        // `plan_agent_spawn_with` 冒頭の `get_session` を模した古いスナップショット。
        // `prepare_worktree` が in-memory に書き込む効果（branch/worktree_path の確定）
        // だけを反映し、その後の DB 書き込みには追随させない。
        let mut stale_session = state.store.get_session(&inserted.id).expect("get_session");
        let updated_at_before_commit = stale_session.updated_at;
        stale_session.branch = Some("session/in-review".to_string());
        stale_session.worktree_path = Some("/tmp/kamux-test-repo/.worktrees/in-review".to_string());
        stale_session.kanban_status = KanbanStatus::Review;

        std::thread::sleep(std::time::Duration::from_millis(5));

        // `plan_agent_spawn_with` の `set_worktree`（契約 §63.4）と、Review への移動
        // （別経路。ここでは既にレビュー中だったと想定した既定挙動の再現）を、
        // `stale_session` を経由せずに DB へ直接書く。
        state
            .store
            .set_worktree(
                &inserted.id,
                "session/in-review",
                "/tmp/kamux-test-repo/.worktrees/in-review",
            )
            .expect("seed worktree");
        state
            .store
            .update_session(
                &inserted.id,
                &SessionPatch {
                    kanban_status: Some(KanbanStatus::Review),
                    ..Default::default()
                },
            )
            .expect("move to review");

        let result = commit_started_session(&state, &mut stale_session).expect("commit");

        let reloaded = state.store.get_session(&inserted.id).expect("get_session");
        assert_eq!(
            result.updated_at, reloaded.updated_at,
            "戻り値は DB の最新行と一致しなければならない（clone() ではなく get_session 由来）"
        );
        assert!(
            result.updated_at > updated_at_before_commit,
            "plan_agent_spawn_with 側の set_worktree などが進めた updated_at が \
             戻り値に反映されていない"
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

    // ---- Task 11: mark_error の許可リスト（契約 §40.3 + §63.5）----
    //
    // `mark_error` は非同期（consumer スレッド）なので、正の主張は成立を待ち、
    // 負の主張は素の sleep で待つ（契約 §69.1 / §69.2）。

    /// 正の主張用。consumer が ❌ を確定させるまで待つ。
    fn wait_for_error_state(state: &AppState, session_id: &str) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if state.runtime.current(session_id) == RuntimeState::Error {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        state.runtime.current(session_id) == RuntimeState::Error
    }

    /// 契約 §40.3 の恒久ハングの入口。**このテストが許可リストの本体を守っている。**
    ///
    /// 生きているセッションのカードを二度押しすると二重起動ガードが
    /// `AppError::InvalidState` を返す。ここで `mark_error` を呼ぶと `running` の上に
    /// `error` が書かれ、`error` 行は `Spawned` しか受け付けず、ガードは PTY が生きて
    /// いる限り `InvalidState` を返し続けるので `Spawned` は永遠に来ない。`PtyExited` も
    /// `error` からは禁止で、`normalize_on_startup` は `{running, waiting_input}` しか
    /// 触らないため再起動でも消えない —— **カードが ❌ のまま永久に固着する。**
    #[test]
    fn plan_agent_spawn_does_not_mark_error_when_the_double_start_guard_rejects() {
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
        let state = crate::state::test_support::app_state(store);
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
        // 生きているセッションが 🟢 であることを模す（❌ が「上書き」になる状況を作る）
        state
            .runtime
            .sender()
            .send(&session.id, StateInput::Spawned);
        assert!(
            {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                let mut seen = false;
                while std::time::Instant::now() < deadline && !seen {
                    seen = state.runtime.current(&session.id) == RuntimeState::Running;
                    if !seen {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }
                seen
            },
            "前提: 二度押しの前にセッションが running になっていること"
        );

        let err = plan(&state, &session.id).expect_err("must reject the double start");
        assert!(matches!(err, AppError::InvalidState(_)), "actual: {err:?}");

        // 負の主張なので待つ条件が作れない（契約 §69.2）
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert_eq!(
            state.runtime.current(&session.id),
            RuntimeState::Running,
            "二重起動ガードの InvalidState で ❌ を書いてはいけない（契約 §40.3 の恒久ハング）"
        );
        let reloaded = state.store.get_session(&session.id).expect("get_session");
        assert_eq!(reloaded.last_runtime_error, None);

        state.pty.kill(&sid).expect("cleanup stub surface");
    }

    /// 許可リストの「呼ばない」側 2 件目: `get_session` の `NotFound`（契約 §40.3）。
    #[test]
    fn plan_agent_spawn_does_not_mark_error_when_the_session_is_not_found() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, _session) =
            build_state_without_worktree("/tmp/kamux-test-repo", SessionMode::InPlace);

        let err = plan(&state, "no-such-session").expect_err("must fail with NotFound");
        assert!(matches!(err, AppError::NotFound(_)), "actual: {err:?}");

        // 負の主張なので待つ条件が作れない（契約 §69.2）
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert_eq!(
            state.runtime.current("no-such-session"),
            RuntimeState::Idle,
            "get_session の NotFound は起動フェーズではない（契約 §40.3）"
        );
    }

    /// 起動フェーズ 1 段目（`resolve_program`）の Err は ❌ になる。
    /// 生 stderr（`AppError` の `Display`）を加工せずそのまま渡すことも固定する
    /// （契約 §2 / §6 / §40.3）。
    #[test]
    fn plan_agent_spawn_marks_error_when_the_binary_cannot_be_resolved() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Claude, None);

        let failing_resolve =
            |_: &str| -> AppResult<PathBuf> { Err(AppError::CliNotFound("claude".to_string())) };
        let err = plan_agent_spawn_with(
            &state,
            &session.id,
            &fake_launch_env(),
            failing_resolve,
            SpawnIntent::Fresh,
        )
        .expect_err("must fail when the binary cannot be resolved");

        assert!(wait_for_error_state(&state, &session.id));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut reloaded = state.store.get_session(&session.id).expect("get_session");
        while std::time::Instant::now() < deadline && reloaded.last_runtime_error.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(5));
            reloaded = state.store.get_session(&session.id).expect("get_session");
        }
        assert_eq!(
            reloaded.last_runtime_error,
            Some(err.to_string()),
            "AppError の Display をそのまま残す（契約 §2 / §6）"
        );
        assert_eq!(reloaded.last_runtime_state, RuntimeState::Error);
    }

    /// **契約 §63.5 の 5 段目**（§40.3 の表は「4 つ」のままだが、`set_worktree` が
    /// spawn より前へ移った分だけ起動フェーズが 1 つ増えている）。
    ///
    /// `set_worktree` は `prepare_worktree` の直後・spawn より前なので、その `Err` の
    /// 時点でセッションは**確実に起動していない** —— §40.3 の判定基準に照らして ❌ の
    /// 対象である。`PRAGMA query_only` は接続ごとの設定で、読みは通し書きだけを
    /// `SQLITE_READONLY` で落とすため、`get_session` / `get_project` を通過させた上で
    /// `set_worktree` だけを決定的に失敗させられる（実 DB を壊さない唯一の seam）。
    #[test]
    fn plan_agent_spawn_marks_error_when_set_worktree_fails() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_dir, state, session, _worktree_path) =
            build_state_with_worktree_session(CliKind::Shell, None);

        state
            .store
            .conn()
            .expect("conn")
            .execute_batch("PRAGMA query_only = ON;")
            .expect("enable query_only");

        let err = plan(&state, &session.id).expect_err("set_worktree must fail");
        assert!(matches!(err, AppError::Db(_)), "actual: {err:?}");

        assert!(
            wait_for_error_state(&state, &session.id),
            "set_worktree の Err は起動フェーズの失敗なので ❌ にする（契約 §63.5）"
        );
    }

    // --- M3-3: ヒューリスティックのライフサイクル ---

    mod heuristics_lifecycle {
        use super::*;
        // 契約 §64.3 の 1 行の観測に使う。**複製せず共有する** —— 同一プロセスに
        // `tracing` の subscriber は 1 人しか立てられない（`payload.rs` の
        // `tests` モジュールの doc）。
        use crate::hooks_srv::payload::tests::{capture_events, CapturedEvent};

        fn wait_until(f: impl Fn() -> bool) -> bool {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                if f() {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            f()
        }

        fn dummy_spec(session: &Session) -> SpawnSpec {
            SpawnSpec {
                surface_id: crate::pty::surface_id(&session.id, SurfaceKind::Agent),
                program: "/bin/cat".to_string(),
                args: Vec::new(),
                cwd: PathBuf::from("/tmp"),
                env: Vec::new(),
                cols: crate::pty::DEFAULT_COLS,
                rows: crate::pty::DEFAULT_ROWS,
            }
        }

        /// `CliKind::Custom`（ヒューリスティック既定オン・hooks 非対象）のセッションを
        /// `running` まで進めた `AppState` を返す。
        fn running_custom_session() -> (tempfile::TempDir, AppState, Session) {
            let (dir, store, session) = build_project_and_session(
                "/tmp/kamux-heuristics",
                SessionMode::InPlace,
                CliKind::Custom,
                Some("my-cli"),
            );
            let state = crate::state::test_support::app_state(store);
            state
                .runtime
                .sender()
                .send(&session.id, StateInput::Spawned);
            assert!(
                wait_until(|| state.runtime.current(&session.id) == RuntimeState::Running),
                "前提が崩れている: セッションが running になっていない"
            );
            (dir, state, session)
        }

        /// **端から端までの観測点**（契約 §96 の陽性の対照）。
        ///
        /// PTY の出力チャンク → `AgentOutputObserver` → `SessionActivity` →
        /// `HeuristicRegistry` の消費ループ → ゲート → `ManagerSink` → 状態機械、
        /// という production の経路が実際に繋がっていることを 1 本で見る。
        /// **spawn へ渡す observer を `None` に戻す変異はここで赤くなる** ——
        /// 緑のままなら Task 13 は端から端まで何も観測していない。
        #[test]
        fn spawning_an_agent_surface_wires_its_output_to_the_state_machine() {
            let (_dir, state, session) = running_custom_session();

            let mut captured: Option<Box<dyn OutputObserver>> = None;
            spawn_agent_surface_with(&state, &session, dummy_spec(&session), |_spec, observer| {
                captured = observer;
                Ok(())
            })
            .expect("spawn");

            let mut observer = captured.expect("PTY へ observer が渡っていない");
            observer.on_chunk(b"continue? \x07"); // 入力待ちの BEL

            assert!(
                wait_until(|| state.runtime.current(&session.id) == RuntimeState::WaitingInput),
                "BEL が状態機械へ届いていない（現在: {:?}）",
                state.runtime.current(&session.id)
            );
        }

        /// spawn 前にレジストリへ登録される。診断行はセッション自身の id で立つ。
        #[test]
        fn spawning_an_agent_surface_registers_the_session() {
            let (_dir, state, session) = running_custom_session();
            assert!(
                state.heuristics.diagnostics().is_empty(),
                "前提が崩れている"
            );

            spawn_agent_surface_with(&state, &session, dummy_spec(&session), |_spec, _obs| Ok(()))
                .expect("spawn");

            let diag = state.heuristics.diagnostics();
            assert_eq!(diag.len(), 1);
            assert_eq!(diag[0].session_id, session.id);
            assert_eq!(diag[0].cli_kind, CliKind::Custom);
        }

        /// spawn が失敗したセッションは読み取りスレッドを持たず、`on_exit` も来ない。
        /// ここで外さないとレジストリに死んだ登録が残り続ける。
        #[test]
        fn a_failed_spawn_leaves_no_registration_behind() {
            let (_dir, state, session) = running_custom_session();

            let err =
                spawn_agent_surface_with(&state, &session, dummy_spec(&session), |_spec, _obs| {
                    Err(AppError::InvalidState("boom".to_string()))
                })
                .expect_err("spawn must fail");
            assert!(matches!(err, AppError::InvalidState(_)), "actual: {err:?}");

            assert!(
                state.heuristics.diagnostics().is_empty(),
                "spawn 失敗の登録が残っている: {:?}",
                state.heuristics.diagnostics()
            );
        }

        /// spawn クロージャが走ったことを示すマーカー。契約 §64.3 の「`PtyManager::spawn`
        /// の**成功直後**」という**順序**は、このマーカーと shim の warn の出現位置の
        /// 前後で見る（`assert` の対象は件数ではなく index である —— 同じスレッドで
        /// 鳴る無関係なイベント（`runtime_state.rs` の `info!` など）が混ざりうる）。
        const SPAWN_MARKER: &str = "test-marker: the injected spawn ran";

        /// shim の 1 行が PTY へ届かなかったときの `warn!` だけを、鳴った順に拾う。
        ///
        /// **メッセージ本文では絞らない**（文言の変更で無音になる形を作らない）。
        /// `session/` と `pty/` の production が出す `tracing` イベントは
        /// `runtime_state.rs` の `info!` 1 つだけで、`surface_id` フィールドを
        /// 持つ `WARN` は本件以外に存在しない。
        fn shim_line_warns(events: &[CapturedEvent]) -> Vec<&CapturedEvent> {
            events
                .iter()
                .filter(|e| e.level == tracing::Level::WARN && e.fields.contains_key("surface_id"))
                .collect()
        }

        /// `cli_kind` と shim の有無を指定して、`spawn_agent_surface_with` を
        /// **成功する** spawn クロージャで通す。戻り値はその結果と、その間に鳴った
        /// `tracing` イベント全件である。
        ///
        /// **戻り値からは書き込みの成否が見えない。** 契約 §153.3（「`PtyManager::write`
        /// の `Err` は §40.3 の「呼ばない」側である」）により、`write` の `Err` は
        /// `spawn_agent_surface_with` の中で握られて `warn!` へ落ち、関数は `Ok` を返す。
        /// 一方 `PtyManager::spawn` は `AppHandle`（Wry 固定。契約 §15）を要求して
        /// テストから起こせず、レジストリに surface は 1 つも居ないので write は必ず
        /// 失敗する。**したがって「書きに行った」「宛先はどこか」「spawn より後か」は
        /// すべてその warn で観測する。** 1 行の**中身**の逐語を守るのは
        /// `shim::SHELL_PATH_LINE` 側のテストである（契約 §64.3）。
        fn spawn_shell_surface_with_shim(
            cli_kind: CliKind,
            shim_dir: Option<&str>,
        ) -> (
            tempfile::TempDir,
            Session,
            AppResult<()>,
            Vec<CapturedEvent>,
        ) {
            let (dir, store, session) = build_project_and_session(
                "/tmp/kamux-shim-line",
                SessionMode::InPlace,
                cli_kind,
                match cli_kind {
                    CliKind::Custom => Some("my-cli"),
                    _ => None,
                },
            );
            let mut state = crate::state::test_support::app_state(store);
            if let Some(shim_dir) = shim_dir {
                state.hooks = Some(crate::hooks_srv::HooksRuntime {
                    socket_path: PathBuf::from("/tmp/kamux-hooks-shimline.sock"),
                    settings_path: PathBuf::from("/tmp/kamux-hooks-shimline.settings.json"),
                    relay_bin: PathBuf::from("/opt/kamux/kamux-relay"),
                    shim_dir: Some(PathBuf::from(shim_dir)),
                });
            }
            let (result, events) = capture_events(|| {
                spawn_agent_surface_with(&state, &session, dummy_spec(&session), |_spec, _obs| {
                    tracing::info!("{}", SPAWN_MARKER);
                    Ok(())
                })
            });
            (dir, session, result, events)
        }

        /// 契約 §64.3: 「`PtyManager::spawn` の成功直後、呼び出し側が
        /// `PtyManager::write` で送る」。**3 消費者（`start_session` /
        /// `resume_session` / `create_scratch_session`）はすべてこの関数を経由する**
        /// ので、置き場所はここ 1 箇所である（`spawn_editor` はこの関数を通らない）。
        ///
        /// 同時に 2 つ見る:
        /// - **宛先が agent サーフェスであること** —— warn の `error` フィールドは
        ///   `PtyManager::write` に**実際に渡した** `surface_id` から生えるので、
        ///   別の id へ書く変異はここで弁別される。
        /// - **spawn の「成功直後」であること** —— spawn クロージャが出すマーカーが
        ///   warn より**前**に鳴っていること。書き込みを `spawn(...)` の手前へ移す
        ///   変異はこの順序で赤になる。
        #[test]
        fn spawning_a_shell_surface_with_the_shim_enabled_writes_the_pty_line_after_the_spawn() {
            let (_dir, session, result, events) =
                spawn_shell_surface_with_shim(CliKind::Shell, Some("/tmp/kamux-shim"));

            result.expect("write の Err は握る（契約 §153.3）");
            let warns = shim_line_warns(&events);
            assert_eq!(warns.len(), 1, "1 行を書きに行っていない: {events:?}");

            let expected = crate::pty::surface_id(&session.id, SurfaceKind::Agent);
            assert_eq!(
                warns[0].fields.get("surface_id").map(String::as_str),
                Some(expected.as_str()),
                "warn が名指しした宛先が agent サーフェスではない: {:?}",
                warns[0]
            );
            assert_eq!(
                warns[0].fields.get("error").map(String::as_str),
                Some(AppError::NotFound(expected.clone()).to_string().as_str()),
                "`PtyManager::write` に渡した宛先が agent サーフェスではない: {:?}",
                warns[0]
            );

            let marker = events
                .iter()
                .position(|e| e.message == SPAWN_MARKER)
                .expect("spawn クロージャが走っていない");
            let warn = events
                .iter()
                .position(|e| std::ptr::eq(e, warns[0]))
                .expect("warn の位置");
            assert!(
                marker < warn,
                "契約 §64.3 の 1 行が spawn の**前**に書かれている: {events:?}"
            );
        }

        /// 契約 §153.3 / Ruling 21-D: **`PtyManager::write` の `Err` は §40.3 の
        /// 「`mark_error` を呼ばない `Err`」側である。** spawn は成功していて PTY は
        /// 生きているので、ここで `?` を使って伝播させると 3 コマンド
        /// （`start_session` / `resume_session` / `create_and_start_scratch_session_with`）
        /// の `mark_error` へ届き、「PTY は動いているのにカードが ❌ になり、
        /// その間ユーザーは再起動もできない」窓が開く。
        ///
        /// **握り潰しではなく縮退である**ことも同時に見る —— `Ok` を返しつつ
        /// `warn!` が必ず 1 件鳴っていること。
        #[test]
        fn a_failed_pty_line_write_is_degraded_to_a_warning_and_does_not_fail_the_spawn() {
            let (_dir, _session, result, events) =
                spawn_shell_surface_with_shim(CliKind::Shell, Some("/tmp/kamux-shim"));

            assert!(
                result.is_ok(),
                "write の Err が伝播している（3 コマンドの mark_error へ届く）: {result:?}"
            );
            assert_eq!(
                shim_line_warns(&events).len(),
                1,
                "無音で握り潰している（warn が鳴っていない）: {events:?}"
            );
        }

        /// 契約 §64.3: 「書くのは shim 有効時 かつ `cli_kind == Shell` のときだけ」。
        /// shim 無効（`state.hooks == None`）では 1 行も書かない。
        ///
        /// **戻り値の `Ok` では見ない** —— 契約 §153.3 の縮退により「書きに行って
        /// 失敗した」も `Ok` になるため、`Ok` を見るだけの assert は恒真である。
        #[test]
        fn spawning_a_shell_surface_without_the_shim_writes_nothing() {
            let (_dir, _session, result, events) =
                spawn_shell_surface_with_shim(CliKind::Shell, None);
            result.expect("shim 無効なら PTY へ 1 行も書かない");
            assert!(
                shim_line_warns(&events).is_empty(),
                "shim 無効なのに 1 行書きに行っている: {events:?}"
            );
        }

        /// 同上の裏側: shim 有効でも `Shell` 以外へは書かない。
        /// ここも戻り値ではなく warn の件数で見る（上と同じ理由）。
        #[test]
        fn spawning_a_non_shell_surface_with_the_shim_enabled_writes_nothing() {
            for cli_kind in [CliKind::Claude, CliKind::Codex, CliKind::Custom] {
                let (_dir, _session, result, events) =
                    spawn_shell_surface_with_shim(cli_kind, Some("/tmp/kamux-shim"));
                result
                    .unwrap_or_else(|e| panic!("cli_kind={cli_kind:?} で Err になっている: {e:?}"));
                assert!(
                    shim_line_warns(&events).is_empty(),
                    "cli_kind={cli_kind:?} へ 1 行書きに行っている: {events:?}"
                );
            }
        }

        /// `stop_session` でも外す。`PtyManager::kill` は冪等で、既に死んでいる
        /// サーフェスでは `on_exit` が二度と来ないため、そこ任せにすると登録が残る。
        #[test]
        fn stopping_a_session_detaches_the_heuristics() {
            let (_dir, state, session) = running_custom_session();
            spawn_agent_surface_with(&state, &session, dummy_spec(&session), |_spec, _obs| Ok(()))
                .expect("spawn");
            assert_eq!(state.heuristics.diagnostics().len(), 1, "前提が崩れている");

            stop_agent_surface(&state, &session.id).expect("stop");

            assert!(
                state.heuristics.diagnostics().is_empty(),
                "stop_session でヒューリスティックが外れていない"
            );
        }
    }

    // --- M3-4 Task 17: create_scratch_session（契約 §29.3）---

    mod scratch_session {
        use super::*;

        fn wait_until(f: impl Fn() -> bool) -> bool {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                if f() {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            f()
        }

        /// `build_state` は repo_path を文字列として保存するだけで実ディレクトリを
        /// 要求しないため、DB 用の TempDir とは別に repo_path 用の TempDir を持つ。
        /// 両方を呼び出し側が保持し続けないと途中でディレクトリごと消える。
        fn project_with_temp_repo() -> (
            tempfile::TempDir,
            tempfile::TempDir,
            AppState,
            crate::model::Project,
        ) {
            let repo = tempfile::tempdir().expect("tempdir for repo_path");
            let (db_dir, state, project) =
                build_state(repo.path().to_str().expect("utf8 repo path"));
            (db_dir, repo, state, project)
        }

        #[test]
        fn plan_scratch_session_fixes_is_scratch_mode_and_cli_kind() {
            // I-4: この経路は login_shell()（$SHELL 読み取り）を踏むため ENV_LOCK で
            // 直列化する（cli_args.rs の ENV_LOCK doc）。
            let _lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (_db_dir, _repo, state, project) = project_with_temp_repo();

            let (session, _spec) =
                plan_scratch_session_with(&state, &project.id, None, 1_000, &fake_launch_env())
                    .expect("plan scratch session");

            assert!(
                session.is_scratch,
                "スクラッチは常に is_scratch: true（契約 §29.1）"
            );
            // I-1: `Session::new_backlog` へ渡す title/description の 2 引数（どちらも
            // &str）は位置引数の取り違えでは気づけない。取り違えたら別物になる具体値
            // で固定する（レビュー処方 P1）。
            assert_eq!(session.title, "Scratch");
            assert_eq!(session.description, "");
            assert_eq!(session.mode, SessionMode::InPlace, "契約 §29.6");
            assert_eq!(session.cli_kind, CliKind::Shell, "契約 §29.3");
            assert_eq!(session.branch, None, "契約 §29.6");
            assert_eq!(session.worktree_path, None, "契約 §29.6");
            assert_eq!(
                session.kanban_status,
                KanbanStatus::Backlog,
                "契約 §29.1: NOT NULL のまま 'backlog'"
            );
        }

        /// cwd が None のときは project.repo_path へフォールバックする（契約 §29.3）。
        #[test]
        fn plan_scratch_session_falls_back_to_the_project_repo_path_when_cwd_is_none() {
            // I-4: login_shell() を踏むため ENV_LOCK で直列化する。
            let _lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (_db_dir, _repo, state, project) = project_with_temp_repo();

            let (_session, spec) =
                plan_scratch_session_with(&state, &project.id, None, 1_000, &fake_launch_env())
                    .expect("plan scratch session");

            assert_eq!(spec.cwd, PathBuf::from(&project.repo_path));
        }

        /// 弁別: `cwd: Some(..)` は project.repo_path とは別のディレクトリへ通ること。
        /// フォールバック値とは明確に別の値を使うことで、「常に project.repo_path を
        /// 使う」変異（cwd を無視する変異）を弁別する。
        #[test]
        fn plan_scratch_session_uses_the_given_cwd_over_the_project_repo_path() {
            // I-4: login_shell() を踏むため ENV_LOCK で直列化する。
            let _lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (_db_dir, _repo, state, project) = project_with_temp_repo();
            let custom = tempfile::tempdir().expect("custom cwd dir");
            assert_ne!(
                custom.path(),
                std::path::Path::new(&project.repo_path),
                "テスト前提: custom は repo_path と別のディレクトリであること"
            );

            let (_session, spec) = plan_scratch_session_with(
                &state,
                &project.id,
                Some(custom.path().to_str().expect("utf8").to_string()),
                1_000,
                &fake_launch_env(),
            )
            .expect("plan scratch session");

            assert_eq!(spec.cwd, custom.path());
        }

        /// 自分で設計した変異: `is_dir()` を `exists()` に緩めると、通常ファイルの cwd が
        /// ここを素通りしてしまう（`workspace.rs` の
        /// `existing_worktree_path_that_is_a_regular_file_errors` と同種の実害。
        /// portable-pty は chdir を試みず黙って $HOME へフォールバックする）。
        #[test]
        fn plan_scratch_session_rejects_a_cwd_that_is_a_regular_file() {
            let (_db_dir, repo, state, project) = project_with_temp_repo();
            let fake_cwd = repo.path().join("not-a-directory");
            std::fs::write(&fake_cwd, b"").expect("write regular file");

            let err = plan_scratch_session_with(
                &state,
                &project.id,
                Some(fake_cwd.to_str().expect("utf8").to_string()),
                1_000,
                &fake_launch_env(),
            )
            .unwrap_err();

            assert!(matches!(err, AppError::InvalidState(_)), "actual: {err:?}");
        }

        /// `plan_scratch_session_with` は spawn の前に DB へ確定させる。
        #[test]
        fn plan_scratch_session_persists_the_row_before_returning() {
            // I-4: login_shell() を踏むため ENV_LOCK で直列化する。
            let _lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (_db_dir, _repo, state, project) = project_with_temp_repo();

            let (session, _spec) =
                plan_scratch_session_with(&state, &project.id, None, 1_000, &fake_launch_env())
                    .expect("plan scratch session");

            let reloaded = state
                .store
                .get_session(&session.id)
                .expect("row must exist in the store");
            assert!(reloaded.is_scratch);
        }

        /// M4: 作成だけでなく start（spawn の呼び出し）まで含めて 1 手であること。
        ///
        /// **`spawn_agent_surface_with` を経由していることも同時に見る。** 裁定 21-B
        /// は「3 消費者がすべてこの関数を経由するので、契約 §64.3 の 1 行は 1 箇所に
        /// 置けばよい」を前提にしている。経由を外して `spawn` を直に呼ぶ形は、
        /// §64.3 の 1 行と M3-3 のヒューリスティック装着を同時に落とす。
        /// `observer` が `Some` なのは `spawn_agent_surface_with` が
        /// `attach_heuristics` の結果を渡すからで、直呼びでは `None` になる。
        #[test]
        fn create_and_start_scratch_session_calls_the_injected_spawn() {
            // I-4: login_shell() を踏むため ENV_LOCK で直列化する。
            let _lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (_db_dir, _repo, state, project) = project_with_temp_repo();
            let mut called = false;

            create_and_start_scratch_session_with(
                &state,
                &project.id,
                None,
                1_000,
                &fake_launch_env(),
                |_spec, observer| {
                    called = true;
                    assert!(
                        observer.is_some(),
                        "spawn_agent_surface_with を経由していない（裁定 21-B の前提が崩れている）"
                    );
                    Ok(())
                },
            )
            .expect("create and start");

            assert!(called, "start（spawn の呼び出し）が起きていない");
        }

        /// spawn 成功後は runtime_state が Spawned を経て Running へ遷移すること。
        #[test]
        fn create_and_start_scratch_session_reaches_running_on_success() {
            // I-4: login_shell() を踏むため ENV_LOCK で直列化する。
            let _lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (_db_dir, _repo, state, project) = project_with_temp_repo();

            let session = create_and_start_scratch_session_with(
                &state,
                &project.id,
                None,
                1_000,
                &fake_launch_env(),
                |_spec, _observer| Ok(()),
            )
            .expect("create and start");

            assert!(
                wait_until(|| state.runtime.current(&session.id) == RuntimeState::Running),
                "現在: {:?}",
                state.runtime.current(&session.id)
            );
        }

        /// 解釈 6（原子性）で選んだ形の固定: spawn が失敗してもロールバックしない。
        /// 行は残り、error 状態になる（`start_session` の spawn 失敗と同じ扱い）。
        #[test]
        fn create_and_start_scratch_session_leaves_the_row_and_marks_error_when_spawn_fails() {
            // I-4: login_shell() を踏むため ENV_LOCK で直列化する。
            let _lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (_db_dir, _repo, state, project) = project_with_temp_repo();

            let err = create_and_start_scratch_session_with(
                &state,
                &project.id,
                None,
                1_000,
                &fake_launch_env(),
                |_spec, _observer| Err(AppError::InvalidState("boom".to_string())),
            )
            .unwrap_err();
            assert!(matches!(err, AppError::InvalidState(_)), "actual: {err:?}");

            let rows = state
                .store
                .list_sessions(&project.id, true)
                .expect("list sessions");
            assert_eq!(rows.len(), 1, "行がロールバックされている（残すのが仕様）");
            assert!(rows[0].is_scratch);

            assert!(
                wait_for_error_state(&state, &rows[0].id),
                "spawn 失敗は error へ遷移するはず"
            );
        }

        /// C1（陽性の対照）: 永続化を飛ばすと、この後の store 読み戻しが検知する。
        #[test]
        fn create_and_start_scratch_session_row_is_readable_after_success() {
            // I-4: login_shell() を踏むため ENV_LOCK で直列化する。
            let _lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (_db_dir, _repo, state, project) = project_with_temp_repo();

            let session = create_and_start_scratch_session_with(
                &state,
                &project.id,
                None,
                1_000,
                &fake_launch_env(),
                |_spec, _observer| Ok(()),
            )
            .expect("create and start");

            let reloaded = state
                .store
                .get_session(&session.id)
                .expect("row must exist in the store after success");
            assert!(reloaded.is_scratch);
            assert_eq!(reloaded.mode, SessionMode::InPlace);
            assert_eq!(reloaded.cli_kind, CliKind::Shell);
            // I-5: start 後も kanban_status は Backlog のまま（§29.1）。
            // `commit_started_session` を呼んで in_progress へ進めると §29.4 の
            // カンバン除外フィルタが漏れた瞬間にカードとして現れる（レビュー処方 P5）。
            assert_eq!(reloaded.kanban_status, KanbanStatus::Backlog);
        }

        /// I-2 の鏡像: agent 経路の `plan_agent_spawn_injects_hooks_settings_and_sock_for_claude`
        /// に対応するスクラッチ経路のテスト。`cli_args.rs:285-287` の doc は逐語で
        /// 「env（KAMUX_HOOKS_SOCK）は全 cli_kind 共通…shell のスクラッチ端末から手で
        /// 起動した claude の hook も relay に届く必要があるため、cli_kind で絞っては
        /// ならない」と書いている。`apply_hooks(&session, launch, state.hooks.as_ref())`
        /// の呼び出し（本ファイル `plan_scratch_session_with` 内）を削っても、この
        /// テストを足す前は全緑だった（レビュー指摘 I-2）。`--settings` は claude 専用
        /// フラグなのでここでは assert しない。
        #[test]
        fn plan_scratch_session_injects_the_hooks_sock_env_even_for_the_shell_cli_kind() {
            // I-4: login_shell() を踏むため ENV_LOCK で直列化する。
            let _lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (_db_dir, _repo, mut state, project) = project_with_temp_repo();
            state.hooks = Some(crate::hooks_srv::HooksRuntime {
                socket_path: PathBuf::from("/tmp/kamux-hooks-scratch.sock"),
                settings_path: PathBuf::from("/tmp/kamux-hooks-scratch.settings.json"),
                relay_bin: PathBuf::from("/opt/kamux/kamux-relay"),
                shim_dir: None,
            });

            let (_session, spec) =
                plan_scratch_session_with(&state, &project.id, None, 1_000, &fake_launch_env())
                    .expect("plan scratch session");

            assert!(
                spec.env.contains(&(
                    "KAMUX_HOOKS_SOCK".to_string(),
                    "/tmp/kamux-hooks-scratch.sock".to_string()
                )),
                "actual env: {:?}",
                spec.env
            );
        }

        /// **スクラッチ経路（`cli_kind == Shell`）の shim 配線の観測**（契約 §30.2 /
        /// §64.5.1）。手打ちした `claude` に `--settings` が届くために必要なものが
        /// 全部揃っていることを 1 本で見る:
        ///
        /// - `KAMUX_HOOKS_SOCK`（relay の宛先）
        /// - `KAMUX_HOOKS_SETTINGS`（shim がこれを見て `--settings` を足す）
        /// - `KAMUX_SHIM_DIR`（shim 自身が PATH から自分を除くために見る）
        /// - `PATH` の**先頭**が shim ディレクトリであること（`{shim_dir}:{現プロセスの
        ///   PATH}`。§30.2 の逐語）
        ///
        /// **3 つのうち 1 つでも欠けると手打ちの `claude` は hook を飛ばさない。**
        /// 既存の `..._injects_the_hooks_sock_env_even_for_the_shell_cli_kind` は
        /// `KAMUX_HOOKS_SOCK` しか見ておらず、shim を落としても全緑だった。
        #[test]
        fn plan_scratch_session_carries_the_whole_shim_env_for_the_shell_cli_kind() {
            let _lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (_db_dir, _repo, mut state, project) = project_with_temp_repo();
            state.hooks = Some(crate::hooks_srv::HooksRuntime {
                socket_path: PathBuf::from("/tmp/kamux-hooks-scratch.sock"),
                settings_path: PathBuf::from("/tmp/kamux-hooks-scratch.settings.json"),
                relay_bin: PathBuf::from("/opt/kamux/kamux-relay"),
                shim_dir: Some(PathBuf::from("/tmp/kamux-scratch-shim")),
            });

            let (_session, spec) =
                plan_scratch_session_with(&state, &project.id, None, 1_000, &fake_launch_env())
                    .expect("plan scratch session");

            for (key, value) in [
                ("KAMUX_HOOKS_SOCK", "/tmp/kamux-hooks-scratch.sock"),
                (
                    "KAMUX_HOOKS_SETTINGS",
                    "/tmp/kamux-hooks-scratch.settings.json",
                ),
                ("KAMUX_SHIM_DIR", "/tmp/kamux-scratch-shim"),
            ] {
                assert!(
                    spec.env.contains(&(key.to_string(), value.to_string())),
                    "{key} が env に無い: {:?}",
                    spec.env
                );
            }

            let paths: Vec<&str> = spec
                .env
                .iter()
                .filter(|(k, _)| k == "PATH")
                .map(|(_, v)| v.as_str())
                .collect();
            // 契約 §30.2: `{shim_dir}:{現プロセスの PATH}`。`launch_env.path`
            // （`/fake/bin`）を土台にする変異はここで赤くなる。
            assert_eq!(
                paths,
                vec![format!(
                    "/tmp/kamux-scratch-shim:{}",
                    std::env::var("PATH").unwrap_or_default()
                )
                .as_str()],
                "PATH の対が 1 つでその先頭が shim ディレクトリであること: {:?}",
                spec.env
            );
        }

        /// I-3 / I-4: `spec.program` にログインシェル自身が入ることを固定する
        /// （直上の production コメント「ログインシェル自身が起動対象になる」の断定
        /// を偽にする変異 `login_shell()` → `"/bin/false"` を弁別する）。既存 agent 経路
        /// の先例 `plan_agent_spawn_uses_the_login_shell_for_custom_cli_kind_not_resolve_program`
        /// と同型。login_shell()（$SHELL 読み取り）を踏むため ENV_LOCK + ShellEnvGuard
        /// で直列化・固定する。
        #[test]
        fn plan_scratch_session_uses_the_login_shell_as_the_program() {
            let _lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let _guard = ShellEnvGuard::set("/tmp/kamux-test-login-shell");
            let (_db_dir, _repo, state, project) = project_with_temp_repo();

            let (_session, spec) =
                plan_scratch_session_with(&state, &project.id, None, 1_000, &fake_launch_env())
                    .expect("plan scratch session");

            assert_eq!(spec.program, "/tmp/kamux-test-login-shell");
        }
    }
}
