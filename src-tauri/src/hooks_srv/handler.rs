//! HookSink の本番実装。
//!
//! M2-1 の RuntimeSender と Store にしか依存しない。SessionManager を経由しないので、
//! テストで PTY を用意する必要がなく、accept スレッドからロック競合なしに呼べる。

use std::sync::Arc;

use crate::hooks_srv::{HookEvent, HookKind, HookSink};
use crate::session::runtime_state::{RuntimeSender, StateInput};
use crate::store::Store;

pub struct HookHandler {
    store: Arc<Store>,
    runtime_tx: RuntimeSender,
}

impl HookHandler {
    pub fn new(store: Arc<Store>, runtime_tx: RuntimeSender) -> Self {
        Self { store, runtime_tx }
    }

    /// DB 上に存在するセッションか。設計 §6-7 のなりすまし防止フィルタ。
    fn is_known_session(&self, session_id: &str) -> bool {
        self.store.get_session(session_id).is_ok()
    }
}

impl HookSink for HookHandler {
    /// hooks_srv の accept スレッドから同期的に呼ばれる。
    /// DB 更新 1 行と mpsc への送信だけなので accept ループを止める時間は無視できる(設計 §6-9)。
    fn on_hook(&self, event: HookEvent) {
        // M2-1 §5.1: 未知 ID のフィルタは呼び出し側の責務。
        // send() する前に弾かないと状態機械のマップにゴミが残る。
        if !self.is_known_session(&event.kamux_session_id) {
            tracing::warn!(session_id = %event.kamux_session_id, "hook for an unknown session, dropped");
            return;
        }

        match event.kind {
            HookKind::SessionStart => {
                // 契約 §12.6: --resume は同じ ID、--continue は新しい ID を返す。
                // source では分岐せず常に上書きする(設計 §6-9)。
                match event.claude_session_id.as_deref() {
                    Some(claude_session_id) => {
                        if let Err(e) = self
                            .store
                            .set_claude_session_id(&event.kamux_session_id, claude_session_id)
                        {
                            tracing::warn!(error = %e, "failed to persist claude_session_id");
                        }
                    }
                    None => {
                        tracing::warn!(
                            source = ?event.source,
                            "SessionStart payload had no session_id; --resume will fall back to --continue"
                        );
                    }
                }
                // 状態機械には送らない。SessionStart に対応する StateInput が存在しない。
            }
            // 契約 §12.4: どちらも waiting_input へ落ちるが、遷移理由は区別する。
            // 潰さない理由: 契約 §8 が StateReason::HookPermission を
            // 「PermissionRequest hook 受信」と定義しており、潰すと
            // StateInput::HookPermission / StateReason::HookPermission /
            // TS の 'hook_permission' がまとめて到達不能になる(2026-08-09 訂正)。
            HookKind::Notification => {
                self.runtime_tx
                    .send(&event.kamux_session_id, StateInput::HookNotification);
            }
            HookKind::PermissionRequest => {
                self.runtime_tx
                    .send(&event.kamux_session_id, StateInput::HookPermission);
            }
            HookKind::Stop => {
                self.runtime_tx
                    .send(&event.kamux_session_id, StateInput::HookStop);
            }
            HookKind::Other(name) => {
                // 契約 §12.2: ユーザー自身の settings.json とマージされるので、
                // 登録していないイベントが届くのは想定内。
                tracing::debug!(hook = %name, "unsubscribed hook kind, ignored");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::model::{CliKind, RuntimeState, SessionStatePayload, StateReason};
    use crate::session::runtime_state::StateObserver;

    /// Store と RuntimeStateManager だけで組む。SessionManager も PTY も要らない。
    ///
    /// `RuntimeStateManager::new()` は consumer スレッドを起動する（M2-1 §4）。
    /// テストごとに 1 本立つので、**各テストの末尾で必ず `runtime.begin_shutdown()` を呼ぶ**。
    /// 呼ばないと `cargo test` の並列実行でスレッドが積み上がる。
    /// TempDir を返り値に含めること。落とすと DB ごと消える（2026-08-09 訂正）。
    fn handler_with_session() -> (
        HookHandler,
        Arc<crate::store::Store>,
        RuntimeSender,
        Arc<crate::session::runtime_state::RuntimeStateManager>,
        String,
        tempfile::TempDir,
    ) {
        let (dir, store) = crate::store::test_support::open_temp();
        let store = Arc::new(store);
        let project = store
            .insert_project("p", "/tmp/p", CliKind::Claude)
            .expect("project");
        let session = crate::store::test_support::insert_test_session(&store, &project.id, "t");

        // RuntimeStateManager::new は Arc<dyn StatePersist> を取るので明示コアーションが要る。
        let runtime = crate::session::runtime_state::RuntimeStateManager::new(
            Arc::clone(&store) as Arc<dyn crate::session::runtime_state::StatePersist>
        );
        let tx = runtime.sender();
        // hook を受ける前提として running にしておく（PTY spawn 相当）。
        tx.send(&session.id, StateInput::Spawned);

        let handler = HookHandler::new(Arc::clone(&store), tx.clone());
        (handler, store, tx, runtime, session.id, dir)
    }

    /// send() は mpsc 経由で consumer スレッドが処理するので、反映を待つ。
    fn wait_state(tx: &RuntimeSender, id: &str, want: RuntimeState) {
        for _ in 0..200 {
            if tx.current(id) == want {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!(
            "state did not become {want:?} within 2s (now {:?})",
            tx.current(id)
        );
    }

    #[test]
    fn session_start_persists_claude_session_id_without_changing_state() {
        let (handler, store, tx, runtime, id, _dir) = handler_with_session();
        wait_state(&tx, &id, RuntimeState::Running);

        handler.on_hook(HookEvent {
            kamux_session_id: id.clone(),
            kind: HookKind::SessionStart,
            claude_session_id: Some("550e8400-e29b-41d4-a716-446655440000".to_string()),
            source: Some("startup".to_string()),
        });

        let got = store.get_session(&id).expect("get");
        assert_eq!(
            got.claude_session_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(
            tx.current(&id),
            RuntimeState::Running,
            "SessionStart must not change runtime_state"
        );

        runtime.begin_shutdown();
    }

    /// 契約 §12.6: --continue 経由の再開では新しい ID が来る。source では分岐しない。
    #[test]
    fn session_start_with_resume_source_also_overwrites() {
        let (handler, store, _tx, runtime, id, _dir) = handler_with_session();
        store.set_claude_session_id(&id, "old-id").expect("seed");

        handler.on_hook(HookEvent {
            kamux_session_id: id.clone(),
            kind: HookKind::SessionStart,
            claude_session_id: Some("new-id".to_string()),
            source: Some("resume".to_string()),
        });

        assert_eq!(
            store
                .get_session(&id)
                .expect("get")
                .claude_session_id
                .as_deref(),
            Some("new-id")
        );

        runtime.begin_shutdown();
    }

    #[test]
    fn session_start_without_session_id_leaves_db_untouched() {
        let (handler, store, _tx, runtime, id, _dir) = handler_with_session();

        handler.on_hook(HookEvent {
            kamux_session_id: id.clone(),
            kind: HookKind::SessionStart,
            claude_session_id: None,
            source: Some("startup".to_string()),
        });

        assert_eq!(store.get_session(&id).expect("get").claude_session_id, None);

        runtime.begin_shutdown();
    }

    #[test]
    fn notification_moves_to_waiting_input() {
        let (handler, _store, tx, runtime, id, _dir) = handler_with_session();

        handler.on_hook(HookEvent {
            kamux_session_id: id.clone(),
            kind: HookKind::Notification,
            claude_session_id: None,
            source: None,
        });

        wait_state(&tx, &id, RuntimeState::WaitingInput);
        runtime.begin_shutdown();
    }

    /// 契約 §12.4: PermissionRequest は waiting_input の最も直接的な信号。
    ///
    /// このテストだけでは HookNotification と HookPermission を区別できない
    /// （どちらも WaitingInput）。区別する観測は下の
    /// `permission_request_and_notification_are_distinguished_by_reason` が持つ。
    #[test]
    fn permission_request_also_moves_to_waiting_input() {
        let (handler, _store, tx, runtime, id, _dir) = handler_with_session();

        handler.on_hook(HookEvent {
            kamux_session_id: id.clone(),
            kind: HookKind::PermissionRequest,
            claude_session_id: None,
            source: None,
        });

        wait_state(&tx, &id, RuntimeState::WaitingInput);
        runtime.begin_shutdown();
    }

    /// 観測用の StateObserver。通知された (session_id, reason) を記録する。
    #[derive(Default)]
    struct RecordingObserver {
        seen: Mutex<Vec<(String, StateReason)>>,
    }

    impl RecordingObserver {
        fn seen(&self) -> Vec<(String, StateReason)> {
            self.seen.lock().expect("lock").clone()
        }
    }

    impl StateObserver for RecordingObserver {
        fn on_state(&self, payload: &SessionStatePayload) {
            self.seen
                .lock()
                .expect("lock")
                .push((payload.session_id.clone(), payload.reason));
        }
    }

    /// consumer スレッド経由の通知が届くまで、有界時間だけ待つ。
    fn wait_observed(observer: &RecordingObserver, want: usize) {
        for _ in 0..200 {
            if observer.seen().len() >= want {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!(
            "observer did not see {want} events within 2s (now {:?})",
            observer.seen()
        );
    }

    /// PermissionRequest → StateInput::HookPermission と
    /// Notification → StateInput::HookNotification は **どちらも RuntimeState::WaitingInput**
    /// へ落ちる。差は StateReason にしか出ない。したがって `tx.current()` を見る
    /// 上記のテストは、実装が両方を HookNotification に潰していても緑になる。
    /// **潰した実装を赤にできる観測が 1 本も無い状態で書き始めない。**
    ///
    /// 両側を assert する:
    ///   1. PermissionRequest を送ると StateReason::HookPermission が観測される
    ///   2. Notification を送ると StateReason::HookPermission は観測されない
    ///      （観測されるのは StateReason::HookNotification）
    ///
    /// **2 つの入力は別々の fixture(= 別セッション)へ送る。** `next_state` は
    /// `(WaitingInput, HookNotification|HookPermission) -> None`(遷移なし)と定めており、
    /// 同一セッションへ両方を続けて送ると 2 回目は WaitingInput → WaitingInput の
    /// 無遷移になって observer が一切呼ばれない(実測で確認済み: 1 件目の
    /// PermissionRequest しか観測されず 2 件目待ちがタイムアウトした)。
    /// 各入力を Running から新規に遷移させるため、セッションを分ける。
    #[test]
    fn permission_request_and_notification_are_distinguished_by_reason() {
        let observer = Arc::new(RecordingObserver::default());

        let (handler_a, _store_a, tx_a, runtime_a, id_a, _dir_a) = handler_with_session();
        wait_state(&tx_a, &id_a, RuntimeState::Running);
        runtime_a.register_observer(observer.clone());
        handler_a.on_hook(HookEvent {
            kamux_session_id: id_a.clone(),
            kind: HookKind::PermissionRequest,
            claude_session_id: None,
            source: None,
        });
        wait_observed(&observer, 1);
        runtime_a.begin_shutdown();

        let (handler_b, _store_b, tx_b, runtime_b, id_b, _dir_b) = handler_with_session();
        wait_state(&tx_b, &id_b, RuntimeState::Running);
        runtime_b.register_observer(observer.clone());
        handler_b.on_hook(HookEvent {
            kamux_session_id: id_b.clone(),
            kind: HookKind::Notification,
            claude_session_id: None,
            source: None,
        });
        wait_observed(&observer, 2);
        runtime_b.begin_shutdown();

        let seen = observer.seen();
        assert_eq!(
            seen[0],
            (id_a, StateReason::HookPermission),
            "PermissionRequest must be observed as StateReason::HookPermission"
        );
        assert_eq!(
            seen[1],
            (id_b, StateReason::HookNotification),
            "Notification must be observed as StateReason::HookNotification, not HookPermission"
        );
    }

    /// 契約 §12.2 のマージにより両方発火しうる。同じ状態への二重遷移は無害。
    #[test]
    fn notification_then_permission_request_stays_waiting_input() {
        let (handler, _store, tx, runtime, id, _dir) = handler_with_session();

        for kind in [HookKind::Notification, HookKind::PermissionRequest] {
            handler.on_hook(HookEvent {
                kamux_session_id: id.clone(),
                kind,
                claude_session_id: None,
                source: None,
            });
        }

        wait_state(&tx, &id, RuntimeState::WaitingInput);
        runtime.begin_shutdown();
    }

    #[test]
    fn stop_moves_to_idle() {
        let (handler, _store, tx, runtime, id, _dir) = handler_with_session();

        handler.on_hook(HookEvent {
            kamux_session_id: id.clone(),
            kind: HookKind::Stop,
            claude_session_id: None,
            source: None,
        });

        wait_state(&tx, &id, RuntimeState::Idle);
        runtime.begin_shutdown();
    }

    /// 設計 §6-7 / M2-1 §5.1: 未知 ID のフィルタは呼び出し側（= ここ）の責務。
    ///
    /// `tx.current(unknown)` は assert しない。M2-1 §5.1 が「未知のセッションは
    /// RuntimeState::Idle」と定めているため、フィルタが効いていても効いていなくても
    /// Idle が返り、何も判別できない（トートロジーになる）。
    /// 代わりに「未知 ID への送信が既知セッションを 1 ミリも動かさない」ことを
    /// 有界待機で確かめる。
    #[test]
    fn unknown_session_id_is_dropped_before_reaching_the_state_machine() {
        let (handler, store, tx, runtime, id, _dir) = handler_with_session();
        let unknown = "00000000-0000-4000-8000-000000000000";
        assert!(
            store.get_session(unknown).is_err(),
            "precondition: unknown must not be in the DB"
        );

        for kind in [HookKind::Notification, HookKind::Stop] {
            handler.on_hook(HookEvent {
                kamux_session_id: unknown.to_string(),
                kind,
                claude_session_id: Some("x".to_string()),
                source: None,
            });
        }

        // 送信が状態機械に届いていれば consumer スレッドが処理する猶予を与える。
        std::thread::sleep(std::time::Duration::from_millis(200));

        // 既知セッションは無傷（= 未知 ID の送信は 1 件も届いていない）
        assert_eq!(tx.current(&id), RuntimeState::Running);
        // DB も無傷
        assert_eq!(store.get_session(&id).expect("get").claude_session_id, None);

        runtime.begin_shutdown();
    }

    /// 契約 §12.2 のマージでユーザー側の hook が増えても壊れない。
    /// PermissionDenied は登録していないのでここに落ちる。
    #[test]
    fn other_hook_kinds_are_ignored() {
        let (handler, store, tx, runtime, id, _dir) = handler_with_session();
        // fixture の Spawned が Running へ処理されるのを待ってから確認する。
        // 待たずに tx.current() を見ると、consumer スレッドがまだ Spawned を
        // 消化していない Idle を観測しうる(このテスト自体は何も送らないので、
        // Other 側の on_hook 呼び出しはこの待機と無関係)。
        wait_state(&tx, &id, RuntimeState::Running);

        for name in ["PreToolUse", "PermissionDenied"] {
            handler.on_hook(HookEvent {
                kamux_session_id: id.clone(),
                kind: HookKind::Other(name.to_string()),
                claude_session_id: Some("x".to_string()),
                source: None,
            });
        }

        assert_eq!(tx.current(&id), RuntimeState::Running);
        assert_eq!(store.get_session(&id).expect("get").claude_session_id, None);

        runtime.begin_shutdown();
    }
}
