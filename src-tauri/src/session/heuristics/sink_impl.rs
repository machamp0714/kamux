//! M3-3 のヒューリスティックを production の状態機械へ結線する層。
//! M2-1 / M1-4 の具象 API（`RuntimeSender` / `Session`）に依存するのはこのファイルだけ。

use std::sync::Arc;

use super::registry::HeuristicRegistry;
use super::{AgentOutputObserver, OutputObserver, RuntimeStateSink};
use crate::model::{RuntimeState, Session};
use crate::session::runtime_state::{RuntimeSender, StateInput};

/// `RuntimeStateSink` の本番実装。**`Weak` を持たず `RuntimeSender` を所有する。**
///
/// 循環参照が存在しないためである —— 保持の向きは
/// `AppState → HeuristicRegistry → ManagerSink → (states, tx)` の一方向で、
/// `HeuristicRegistry` を指すのは `AppState` だけである。守るべき循環が無いのに
/// `Weak` を置くと、`upgrade()` 失敗という到達不能で無音の失敗経路が 1 本増えるだけになる。
///
/// 先例は `HookHandler::new(store, runtime_tx)`（`hooks_srv/handler.rs`）——
/// 同じ層・同じ目的で `RuntimeSender` を所有している。`RuntimeSender` は
/// `Arc<RwLock<StateMap>>` と `EventTx` を持つだけの clone 可能なハンドルである。
pub(crate) struct ManagerSink(RuntimeSender);

impl ManagerSink {
    pub(crate) fn new(runtime_tx: RuntimeSender) -> Self {
        Self(runtime_tx)
    }
}

impl RuntimeStateSink for ManagerSink {
    /// **本番では常に `Some` を返す。** `RuntimeSender::current` は未知のセッションに
    /// `Idle` を返す（`Option` ではない）ので、`None` になる入力が存在しない。
    ///
    /// それでも `Option` を外さないのは、この `Option` が trait 側の契約
    /// （Task 6 で着地済み）であり、`FakeSink` は未登録のセッションに `None` を返す ——
    /// 消費ループの `None` 枝（登録が無い＝評価しない）はそちらで生きているためである。
    fn current(&self, session_id: &str) -> Option<RuntimeState> {
        Some(self.0.current(session_id))
    }

    /// 契約 §41.4 / M2-1 §5.1: 渡すのは**入力**である。次の状態と
    /// `session://state/{session_id}` の発行は M2-1 の consumer スレッドが決める。
    fn send(&self, session_id: &str, input: StateInput) {
        self.0.send(session_id, input);
    }
}

/// PTY spawn の直前に呼ぶ。返したオブザーバを agent サーフェスの読み取りスレッドへ渡す。
///
/// **`Session` の 3 フィールドをそのまま流し込むのがこの関数の全仕事である。**
/// 既定値の決定（`cli_kind` ごとのオン/オフ）は `Session::new_backlog` が済ませており、
/// ここで再計算すると DB と実行時設定が食い違う。
pub(crate) fn attach_heuristics(
    registry: &Arc<HeuristicRegistry>,
    session: &Session,
) -> Box<dyn OutputObserver> {
    let activity = registry.register(
        &session.id,
        session.cli_kind,
        session.heuristics_enabled,
        session.silence_timeout_secs,
    );
    Box::new(AgentOutputObserver::new(activity))
}

/// PTY exit / `stop_session` で呼ぶ。未登録の `session_id` では no-op。
///
/// **キーは `session_id` であって `surface_id` ではない。** `surface_id` から呼ぶ側は
/// `crate::pty::agent_session_id` で agent サーフェスに限ってから渡すこと ——
/// `s1:editor` の終了でここへ来ると、nvim を閉じただけで agent 側の沈黙推定が死ぬ。
pub(crate) fn detach_heuristics(registry: &Arc<HeuristicRegistry>, session_id: &str) {
    registry.unregister(session_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CliKind, RuntimeState, Session, SessionMode};
    use crate::session::heuristics::clock::TestClock;
    use crate::session::heuristics::hook_liveness::HookLiveness;
    use crate::session::heuristics::registry::HeuristicRegistry;
    use crate::session::heuristics::{FakeSink, OutputObserver};
    use crate::session::runtime_state::StateInput;
    use std::sync::Arc;
    use std::time::Duration;

    async fn advance(clock: &TestClock, ms: u64) {
        clock.advance_ms(ms as i64);
        tokio::time::advance(Duration::from_millis(ms)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
    }

    /// `attach_heuristics` へ渡す `Session`。**`project_id` は `id` と必ず別物にする** ——
    /// 同じ素の型（`String`）の隣接フィールドなので、`register` へ渡す側が取り違えても
    /// 名前でしか判別できない（契約 §81.2）。別物にしておけば診断行の `session_id` で
    /// 取り違えが赤くなる。
    fn session_with(
        session_id: &str,
        cli_kind: CliKind,
        enabled: bool,
        timeout_secs: u32,
    ) -> Session {
        let mut session = Session::new_backlog(
            "project-of-s1",
            "title",
            "",
            SessionMode::InPlace,
            None,
            cli_kind,
            None,
            0.0,
            0,
        );
        session.id = session_id.to_string();
        session.heuristics_enabled = enabled;
        session.silence_timeout_secs = timeout_secs;
        session
    }

    /// `attach_heuristics` の呼び出しラッパ。**production の自由関数をそのまま通す** ——
    /// ここで `register` を組み直すと、下の 6 本は「production が同じ組み立てをしている」
    /// ことを 1 ミリも測らなくなる（群 S）。
    fn attach(
        reg: &Arc<HeuristicRegistry>,
        session_id: &str,
        cli_kind: CliKind,
        enabled: bool,
        timeout_secs: u32,
    ) -> Box<dyn OutputObserver> {
        attach_heuristics(
            reg,
            &session_with(session_id, cli_kind, enabled, timeout_secs),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn a_generic_cli_session_goes_running_then_idle() {
        let clock = TestClock::new(0);
        let sink = Arc::new(FakeSink::new(&[("s1", RuntimeState::Running)]));
        let reg = HeuristicRegistry::new(
            Arc::new(clock.clone()),
            sink.clone(),
            tokio::runtime::Handle::current(),
        );

        let mut obs = attach(&reg, "s1", CliKind::Custom, true, 30);
        obs.on_chunk(b"cargo build\n");
        tokio::task::yield_now().await;

        advance(&clock, 31_000).await;
        assert_eq!(
            sink.sent(),
            vec![("s1".to_string(), StateInput::SilenceTimeout)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_bel_then_silence_keeps_waiting_input() {
        let clock = TestClock::new(0);
        let sink = Arc::new(FakeSink::new(&[("s1", RuntimeState::Running)]));
        let reg = HeuristicRegistry::new(
            Arc::new(clock.clone()),
            sink.clone(),
            tokio::runtime::Handle::current(),
        );

        let mut obs = attach(&reg, "s1", CliKind::Custom, true, 30);
        obs.on_chunk(b"continue? \x07");
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert_eq!(sink.current("s1"), Some(RuntimeState::WaitingInput));

        advance(&clock, 120_000).await;
        assert_eq!(
            sink.current("s1"),
            Some(RuntimeState::WaitingInput),
            "沈黙で塗り潰さない"
        );
        assert_eq!(sink.sent().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_claude_session_receiving_hooks_never_falls_back() {
        let clock = TestClock::new(0);
        let sink = Arc::new(FakeSink::new(&[("s1", RuntimeState::Running)]));
        let reg = HeuristicRegistry::new(
            Arc::new(clock.clone()),
            sink.clone(),
            tokio::runtime::Handle::current(),
        );

        let mut obs = attach(&reg, "s1", CliKind::Claude, true, 30);
        obs.on_chunk(b"starting claude\n");
        reg.note_hook("s1"); // SessionStart hook
        tokio::task::yield_now().await;

        // 思考中に 2 分出力が止まっても idle にしない
        advance(&clock, 120_000).await;
        assert!(sink.sent().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_claude_session_with_broken_hooks_falls_back_to_heuristics() {
        // 設計書 §12「hooks 不達（設定不備・ソケット断）→ 汎用ヒューリスティックへ自動フォールバック」
        let clock = TestClock::new(0);
        let sink = Arc::new(FakeSink::new(&[("s1", RuntimeState::Running)]));
        let reg = HeuristicRegistry::new(
            Arc::new(clock.clone()),
            sink.clone(),
            tokio::runtime::Handle::current(),
        );

        let mut obs = attach(&reg, "s1", CliKind::Claude, true, 30);
        obs.on_chunk(b"starting claude\n");
        tokio::task::yield_now().await;

        advance(&clock, 31_000).await;
        assert_eq!(
            sink.sent(),
            vec![("s1".to_string(), StateInput::SilenceTimeout)]
        );
        let diag = reg.diagnostics();
        assert_eq!(diag[0].liveness, HookLiveness::Unreachable);
        assert!(diag[0].heuristics_active);
    }

    #[tokio::test(start_paused = true)]
    async fn detaching_on_exit_stops_the_heuristics() {
        let clock = TestClock::new(0);
        let sink = Arc::new(FakeSink::new(&[("s1", RuntimeState::Running)]));
        let reg = HeuristicRegistry::new(
            Arc::new(clock.clone()),
            sink.clone(),
            tokio::runtime::Handle::current(),
        );

        let mut obs = attach(&reg, "s1", CliKind::Custom, true, 30);
        obs.on_chunk(b"work\n");
        tokio::task::yield_now().await;

        detach_heuristics(&reg, "s1"); // PTY exit 相当
        advance(&clock, 120_000).await;
        assert!(sink.sent().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_shell_session_defaults_to_disabled() {
        let clock = TestClock::new(0);
        let sink = Arc::new(FakeSink::new(&[("s1", RuntimeState::Running)]));
        let reg = HeuristicRegistry::new(
            Arc::new(clock.clone()),
            sink.clone(),
            tokio::runtime::Handle::current(),
        );

        let enabled =
            crate::session::heuristics::registry::default_heuristics_enabled(CliKind::Shell);
        let mut obs = attach(&reg, "s1", CliKind::Shell, enabled, 30);
        obs.on_chunk(b"completion failed\x07"); // 補完失敗のベル
        tokio::task::yield_now().await;

        advance(&clock, 120_000).await;
        assert!(sink.sent().is_empty(), "shell は既定オフ");
    }

    // --- `attach_heuristics` が `Session` の**どのフィールド**を渡しているか ---

    /// レジストリのキーは `session.id` であって `session.project_id` ではない。
    /// 両方 `String` で `register` の第 1 引数は `&str` なので、取り違えても
    /// コンパイルは通る（契約 §81.2）。診断行の `session_id` がその観測点である。
    #[tokio::test(start_paused = true)]
    async fn attach_registers_under_the_session_id_not_the_project_id() {
        let sink = Arc::new(FakeSink::new(&[("s1", RuntimeState::Running)]));
        let reg = HeuristicRegistry::new(
            Arc::new(TestClock::new(0)),
            sink,
            tokio::runtime::Handle::current(),
        );

        let session = session_with("s1", CliKind::Custom, true, 30);
        assert_ne!(session.project_id, session.id, "前提が崩れている");
        let _obs = attach_heuristics(&reg, &session);

        let diag = reg.diagnostics();
        assert_eq!(diag.len(), 1);
        assert_eq!(diag[0].session_id, "s1");
        assert_eq!(diag[0].cli_kind, CliKind::Custom);
    }

    /// `heuristics_enabled` は `Session` の値がそのまま届く。
    /// **`true` を直書きする変異はここで赤になる。**
    #[tokio::test(start_paused = true)]
    async fn attach_passes_the_sessions_heuristics_switch() {
        let sink = Arc::new(FakeSink::new(&[("s1", RuntimeState::Running)]));
        let reg = HeuristicRegistry::new(
            Arc::new(TestClock::new(0)),
            sink,
            tokio::runtime::Handle::current(),
        );

        let _off = attach_heuristics(&reg, &session_with("s1", CliKind::Custom, false, 30));
        let _on = attach_heuristics(&reg, &session_with("s2", CliKind::Custom, true, 30));

        let diag = reg.diagnostics();
        let off = diag.iter().find(|s| s.session_id == "s1").expect("s1");
        let on = diag.iter().find(|s| s.session_id == "s2").expect("s2");
        assert!(!off.heuristics_active, "オフのセッションが有効になっている");
        assert!(on.heuristics_active, "オンのセッションが無効になっている");
    }

    /// `silence_timeout_secs` は `Session` の値がそのまま届く。
    /// 既定（30 秒）を直書きする変異は、5.5 秒での発火が消えて赤になる。
    #[tokio::test(start_paused = true)]
    async fn attach_passes_the_sessions_silence_timeout() {
        let clock = TestClock::new(0);
        let sink = Arc::new(FakeSink::new(&[("s1", RuntimeState::Running)]));
        let reg = HeuristicRegistry::new(
            Arc::new(clock.clone()),
            sink.clone(),
            tokio::runtime::Handle::current(),
        );

        let mut obs = attach_heuristics(&reg, &session_with("s1", CliKind::Custom, true, 5));
        obs.on_chunk(b"work\n");
        tokio::task::yield_now().await;

        advance(&clock, 5_500).await;
        assert_eq!(
            sink.sent(),
            vec![("s1".to_string(), StateInput::SilenceTimeout)],
            "セッション固有の 5 秒ではなく既定値が使われている"
        );
    }

    // --- `ManagerSink` が本物の状態機械に載っていること ---

    mod manager_sink {
        use super::*;
        use crate::error::AppResult;
        use crate::session::runtime_state::{RuntimeStateManager, StatePersist};

        /// DB を持たない `StatePersist`。ここで測るのは `ManagerSink` の 2 メソッドが
        /// `RuntimeSender` へ届くことだけで、永続化は関係ない。
        struct NoPersist;

        impl StatePersist for NoPersist {
            fn set_last_runtime_state(&self, _id: &str, _state: RuntimeState) -> AppResult<()> {
                Ok(())
            }
            fn list_ids_by_last_runtime_state(
                &self,
                _state: RuntimeState,
            ) -> AppResult<Vec<String>> {
                Ok(Vec::new())
            }
            fn mark_first_started(&self, _id: &str) -> AppResult<()> {
                Ok(())
            }
            fn set_runtime_error(&self, _id: &str, _message: &str) -> AppResult<()> {
                Ok(())
            }
        }

        fn wait_until(f: impl Fn() -> bool) -> bool {
            for _ in 0..500 {
                if f() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        }

        /// `current` は `RuntimeSender::current` をそのまま返す。未知のセッションでも
        /// `None` にならない（`RuntimeSender::current` が `Idle` を返すため）。
        #[test]
        fn manager_sink_reports_the_state_machines_current_state() {
            let mgr = RuntimeStateManager::new(Arc::new(NoPersist));
            let sink = ManagerSink::new(mgr.sender());

            assert_eq!(sink.current("never-seen"), Some(RuntimeState::Idle));

            mgr.sender().send("s1", StateInput::Spawned);
            assert!(
                wait_until(|| sink.current("s1") == Some(RuntimeState::Running)),
                "状態機械が進めた状態が ManagerSink から見えない"
            );
            mgr.begin_shutdown();
        }

        /// `send` は**入力**を状態機械へ渡す（契約 §41.4）。次の状態は遷移表が決める。
        #[test]
        fn manager_sink_forwards_inputs_to_the_state_machine() {
            let mgr = RuntimeStateManager::new(Arc::new(NoPersist));
            let sink = ManagerSink::new(mgr.sender());

            sink.send("s1", StateInput::Spawned);
            assert!(
                wait_until(|| mgr.current("s1") == RuntimeState::Running),
                "ManagerSink::send が状態機械へ届いていない"
            );

            // `running` からの `SilenceTimeout` は `idle` へ。次の状態を
            // `ManagerSink` 側で決めていないことの観測点でもある
            sink.send("s1", StateInput::SilenceTimeout);
            assert!(
                wait_until(|| mgr.current("s1") == RuntimeState::Idle),
                "SilenceTimeout が遷移表を通っていない"
            );
            mgr.begin_shutdown();
        }
    }
}
