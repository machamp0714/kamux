//! 通知（macOS 通知・Dock バッジ）関連モジュール。
//!
//! `policy` サブモジュールが判定ロジックの型と純粋関数を持つ。
//! OS API に触れる実装は後続タスクで別サブモジュールとして追加する。

pub mod policy;

use std::sync::{Arc, Mutex};

use std::collections::{BTreeMap, HashMap};

use crate::model::{RuntimeState, SessionStatePayload, StateReason};
use policy::{badge_count, decide, format_notification, DecisionInput};

pub use policy::{
    NotifyDecision, NotifyKind, NotifyPermission, SessionLabel, ViewKind, VisibilityContext,
};

/// 1 件の通知の送信要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRequest {
    pub session_id: String,
    pub kind: NotifyKind,
    pub title: String,
    pub body: String,
}

/// OS 通知の送信先。テストでは [`RecordingSink`] に差し替える。
pub trait NotificationSink: Send + Sync + 'static {
    /// 通知を出す。同一 `session_id` の既存通知は置き換える。
    fn post(&self, req: NotificationRequest);
    /// 表示中の通知を取り下げる。該当が無ければ何もしない。
    fn dismiss(&self, session_id: &str);
}

/// 通知がクリックされたときに `session_id` を受け取るコールバック。
pub type ClickHandler = Arc<dyn Fn(&str) + Send + Sync + 'static>;

/// `session_id` から通知文言の材料を引く関数。`Store` への依存をここに閉じ込める。
pub type LabelResolver = Arc<dyn Fn(&str) -> Option<SessionLabel> + Send + Sync + 'static>;

/// テスト用の [`NotificationSink`]。送信内容を記録するだけで OS には触らない。
#[derive(Debug, Default)]
pub struct RecordingSink {
    posted: Mutex<Vec<NotificationRequest>>,
    dismissed: Mutex<Vec<String>>,
}

impl RecordingSink {
    pub fn posted(&self) -> Vec<NotificationRequest> {
        self.posted.lock().expect("RecordingSink poisoned").clone()
    }

    pub fn dismissed(&self) -> Vec<String> {
        self.dismissed
            .lock()
            .expect("RecordingSink poisoned")
            .clone()
    }
}

impl NotificationSink for RecordingSink {
    fn post(&self, req: NotificationRequest) {
        self.posted
            .lock()
            .expect("RecordingSink poisoned")
            .push(req);
    }

    fn dismiss(&self, session_id: &str) {
        self.dismissed
            .lock()
            .expect("RecordingSink poisoned")
            .push(session_id.to_string());
    }
}

/// [`Notifier::on_state`] の結果。副作用（OS 通知の送信）は `Notifier` が済ませており、
/// ここに残るのは呼び出し側（Tauri glue）が実行すべきものだけ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyOutcome {
    /// 通知を出したか、出さなかったならなぜか。
    pub decision: NotifyDecision,
    /// 適用すべき Dock バッジ数。`None` はバッジを消す。
    pub badge: Option<i64>,
    /// 通知許可を要求すべきか（アプリ全体で 1 回だけ true になる）。
    pub request_permission: bool,
}

#[derive(Default)]
struct NotifierInner {
    /// セッションごとの直前 `runtime_state`（エッジトリガ判定用）。
    prev_state: HashMap<String, RuntimeState>,
    /// セッションごとの前回通知時刻（レート制限用）。
    last_notified_at: HashMap<String, i64>,
    /// 現在の `runtime_state` 全体（バッジ計算用）。
    runtime_states: BTreeMap<String, RuntimeState>,
    visibility: VisibilityContext,
    permission: NotifyPermission,
    /// 許可要求を既に払い出したか。
    permission_requested: bool,
}

/// 状態遷移を受け取り、通知の送信と Dock バッジ数の算出を行う。
pub struct Notifier {
    sink: Arc<dyn NotificationSink>,
    labels: LabelResolver,
    inner: Mutex<NotifierInner>,
}

impl Notifier {
    pub fn new(sink: Arc<dyn NotificationSink>, labels: LabelResolver) -> Self {
        Self {
            sink,
            labels,
            inner: Mutex::new(NotifierInner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, NotifierInner> {
        // poison は他スレッドの panic 時のみ。状態が壊れていても通知が 1 回ずれるだけなので回復する
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_permission(&self, p: NotifyPermission) {
        let mut inner = self.lock();
        inner.permission = p;
        if p != NotifyPermission::Unknown {
            inner.permission_requested = true;
        }
    }

    pub fn permission(&self) -> NotifyPermission {
        self.lock().permission
    }

    pub fn set_window_focused(&self, focused: bool) {
        self.lock().visibility.window_focused = focused;
    }

    pub fn set_view_context(&self, view: ViewKind, visible_session_ids: Vec<String>) {
        let mut inner = self.lock();
        inner.visibility.view = Some(view);
        inner.visibility.visible_session_ids = visible_session_ids;
    }

    /// 起動時に DB の `last_runtime_state` からバッジを初期化する。
    pub fn seed_states(&self, states: BTreeMap<String, RuntimeState>) -> Option<i64> {
        let mut inner = self.lock();
        inner.runtime_states = states;
        badge_count(&inner.runtime_states)
    }

    /// セッションが削除・アーカイブされたときに呼ぶ。新しいバッジ数を返す。
    pub fn forget_session(&self, session_id: &str) -> Option<i64> {
        let mut inner = self.lock();
        inner.prev_state.remove(session_id);
        inner.last_notified_at.remove(session_id);
        inner.runtime_states.remove(session_id);
        let badge = badge_count(&inner.runtime_states);
        drop(inner);
        self.sink.dismiss(session_id);
        badge
    }

    /// 状態遷移 1 件を処理する。通知の送信までここで済ませる。
    pub fn on_state(&self, payload: &SessionStatePayload, now_ms: i64) -> NotifyOutcome {
        let session_id = payload.session_id.as_str();
        let next = payload.runtime_state;

        let mut inner = self.lock();
        let prev = inner.prev_state.get(session_id).copied();
        let last_notified_at_ms = inner.last_notified_at.get(session_id).copied();

        let decision = decide(&DecisionInput {
            session_id,
            prev,
            next,
            reason: payload.reason,
            permission: inner.permission,
            visibility: &inner.visibility,
            last_notified_at_ms,
            now_ms,
        });

        let request_permission = payload.reason == StateReason::Spawned
            // この項は set_permission が permission_requested を立てるのと冗長であり、
            // 公開 API 経由では単独では到達不能なのでテストで守れない（PR 18 単位レビュー I-1）。
            // 設計 §5.6 の「権限が未知のときに 1 回だけ訊く」を式の上に残すために意図的に置いている。
            // ⚠️ PR 19 で set_permission の呼び出し方が増える（read_permission() の結果を流す）。
            //    その時点で「公開 API 経由では到達不能」の前提を再検証すること。
            //    Granted → Unknown → Spawned の経路が実際に発生しうる。
            && inner.permission == NotifyPermission::Unknown
            && !inner.permission_requested;
        if request_permission {
            inner.permission_requested = true;
        }

        if let NotifyDecision::Post(_) = decision {
            inner
                .last_notified_at
                .insert(session_id.to_string(), now_ms);
        }
        inner.prev_state.insert(session_id.to_string(), next);
        inner.runtime_states.insert(session_id.to_string(), next);
        let badge = badge_count(&inner.runtime_states);
        drop(inner);

        match decision {
            NotifyDecision::Post(kind) => {
                let label =
                    (self.labels)(session_id).unwrap_or_else(|| SessionLabel::fallback(session_id));
                let (title, body) = format_notification(kind, &label);
                self.sink.post(NotificationRequest {
                    session_id: session_id.to_string(),
                    kind,
                    title,
                    body,
                });
            }
            // 新しい通知を出さないまま waiting_input を抜けたら、古いバナーは嘘になるので取り下げる
            _ if prev == Some(RuntimeState::WaitingInput) && next != RuntimeState::WaitingInput => {
                self.sink.dismiss(session_id);
            }
            _ => {}
        }

        NotifyOutcome {
            decision,
            badge,
            request_permission,
        }
    }
}

#[cfg(test)]
mod sink_tests {
    use super::*;
    use crate::notify::policy::NotifyKind;

    #[test]
    fn recording_sink_captures_posts_in_order() {
        let sink = RecordingSink::default();
        sink.post(NotificationRequest {
            session_id: "s1".into(),
            kind: NotifyKind::WaitingInput,
            title: "入力待ち: a".into(),
            body: "kamux · main".into(),
        });
        sink.post(NotificationRequest {
            session_id: "s2".into(),
            kind: NotifyKind::Stopped,
            title: "応答完了: b".into(),
            body: "kamux · feature/x".into(),
        });

        let posted = sink.posted();
        assert_eq!(posted.len(), 2);
        assert_eq!(posted[0].session_id, "s1");
        assert_eq!(posted[0].kind, NotifyKind::WaitingInput);
        assert_eq!(posted[0].body, "kamux · main");
        assert_eq!(posted[1].title, "応答完了: b");
    }

    #[test]
    fn recording_sink_captures_dismissals() {
        let sink = RecordingSink::default();
        sink.dismiss("s1");
        sink.dismiss("s2");
        assert_eq!(sink.dismissed(), vec!["s1".to_string(), "s2".to_string()]);
    }

    #[test]
    fn recording_sink_is_usable_as_a_trait_object() {
        let sink: Arc<dyn NotificationSink> = Arc::new(RecordingSink::default());
        sink.dismiss("s1");
        // trait 越しに呼べればよい（ダウンキャストはしない）
    }
}

#[cfg(test)]
mod notifier_tests {
    use super::*;
    use crate::model::{RuntimeState, SessionStatePayload, StateReason};
    use crate::notify::policy::NOTIFY_MIN_INTERVAL_MS;
    use std::collections::BTreeMap;

    fn label_of(id: &str) -> SessionLabel {
        SessionLabel {
            title: format!("task-{id}"),
            project_name: "kamux".into(),
            location: "session/x".into(),
        }
    }

    fn fixture() -> (Arc<RecordingSink>, Notifier) {
        let sink = Arc::new(RecordingSink::default());
        let labels: LabelResolver = Arc::new(|id: &str| Some(label_of(id)));
        let notifier = Notifier::new(sink.clone() as Arc<dyn NotificationSink>, labels);
        (sink, notifier)
    }

    fn payload(id: &str, next: RuntimeState, reason: StateReason) -> SessionStatePayload {
        SessionStatePayload {
            session_id: id.to_string(),
            runtime_state: next,
            reason,
        }
    }

    #[test]
    fn waiting_input_posts_a_notification_and_sets_the_badge() {
        let (sink, n) = fixture();
        n.set_permission(NotifyPermission::Granted);
        n.on_state(
            &payload("s1", RuntimeState::Running, StateReason::Spawned),
            0,
        );

        let out = n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_000,
        );

        assert_eq!(out.decision, NotifyDecision::Post(NotifyKind::WaitingInput));
        assert_eq!(out.badge, Some(1));
        let posted = sink.posted();
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].title, "入力待ち: task-s1");
        assert_eq!(posted[0].body, "kamux · session/x");
    }

    #[test]
    fn stop_hook_replaces_the_notification_and_clears_the_badge() {
        let (sink, n) = fixture();
        n.set_permission(NotifyPermission::Granted);
        n.on_state(
            &payload("s1", RuntimeState::Running, StateReason::Spawned),
            0,
        );
        n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_000,
        );

        let out = n.on_state(
            &payload("s1", RuntimeState::Idle, StateReason::HookStop),
            1_000 + NOTIFY_MIN_INTERVAL_MS,
        );

        assert_eq!(out.decision, NotifyDecision::Post(NotifyKind::Stopped));
        assert_eq!(out.badge, None);
        assert_eq!(sink.posted().len(), 2);
        // 置き換えなので dismiss は呼ばない
        assert!(sink.dismissed().is_empty());
    }

    #[test]
    fn leaving_waiting_input_without_a_new_notification_dismisses_the_banner() {
        let (sink, n) = fixture();
        n.set_permission(NotifyPermission::Granted);
        n.on_state(
            &payload("s1", RuntimeState::Running, StateReason::Spawned),
            0,
        );
        n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_000,
        );

        let out = n.on_state(
            &payload("s1", RuntimeState::Exited, StateReason::PtyExited),
            2_000,
        );

        assert_eq!(out.decision, NotifyDecision::SuppressIrrelevantState);
        assert_eq!(out.badge, None);
        assert_eq!(sink.dismissed(), vec!["s1".to_string()]);
    }

    #[test]
    fn a_visible_session_does_not_post_but_still_updates_the_badge() {
        let (sink, n) = fixture();
        n.set_permission(NotifyPermission::Granted);
        n.set_window_focused(true);
        n.set_view_context(ViewKind::Terminal, vec!["s1".into()]);
        n.on_state(
            &payload("s1", RuntimeState::Running, StateReason::Spawned),
            0,
        );

        let out = n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_000,
        );

        assert_eq!(out.decision, NotifyDecision::SuppressVisible);
        assert_eq!(out.badge, Some(1));
        assert!(sink.posted().is_empty());
    }

    #[test]
    fn a_denied_permission_still_updates_the_badge() {
        let (sink, n) = fixture();
        n.set_permission(NotifyPermission::Denied);
        n.on_state(
            &payload("s1", RuntimeState::Running, StateReason::Spawned),
            0,
        );

        let out = n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_000,
        );

        assert_eq!(out.decision, NotifyDecision::SuppressPermissionDenied);
        assert_eq!(out.badge, Some(1));
        assert!(sink.posted().is_empty());
    }

    #[test]
    fn permission_request_after_notification_does_not_double_post() {
        // 契約 §12.4: Notification と PermissionRequest の両方が waiting_input に流れる。
        // さらに §12.2 でユーザー自身の hook も同時発火しうるので、
        // 1 回の入力待ちに hook が複数届くのは既定の挙動。
        let (sink, n) = fixture();
        n.set_permission(NotifyPermission::Granted);
        n.on_state(
            &payload("s1", RuntimeState::Running, StateReason::Spawned),
            0,
        );
        n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_000,
        );

        let out = n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookPermission,
            ),
            1_050,
        );

        assert_eq!(out.decision, NotifyDecision::SuppressNotTransition);
        assert_eq!(out.badge, Some(1));
        assert_eq!(sink.posted().len(), 1);
        // 🟡 のままなので取り下げもしない
        assert!(sink.dismissed().is_empty());
    }

    #[test]
    fn answering_and_being_asked_again_immediately_posts_only_once() {
        // waiting_input → running の唯一の経路は UserInput（M2-1 が出力活動での復帰を禁止した）。
        // ユーザーが答えた直後にまた承認を求められるケースをレート制限が吸収する。
        let (sink, n) = fixture();
        n.set_permission(NotifyPermission::Granted);
        n.on_state(
            &payload("s1", RuntimeState::Running, StateReason::Spawned),
            0,
        );
        n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_000,
        );
        n.on_state(
            &payload("s1", RuntimeState::Running, StateReason::UserInput),
            1_500,
        );
        let out = n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookPermission,
            ),
            2_000,
        );

        assert_eq!(out.decision, NotifyDecision::SuppressRateLimited);
        assert_eq!(sink.posted().len(), 1);
    }

    #[test]
    fn badge_counts_two_waiting_sessions() {
        let (_sink, n) = fixture();
        let first = n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_000,
        );
        // StateReason::Spawned 以外では許可要求を出さない（設計 §5.6）。
        assert!(!first.request_permission);
        let out = n.on_state(
            &payload(
                "s2",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_100,
        );
        assert_eq!(out.badge, Some(2));
    }

    #[test]
    fn seed_states_primes_the_badge_at_startup() {
        let (_sink, n) = fixture();
        let mut m = BTreeMap::new();
        m.insert("s1".to_string(), RuntimeState::Interrupted);
        m.insert("s2".to_string(), RuntimeState::WaitingInput);
        assert_eq!(n.seed_states(m), Some(1));
    }

    #[test]
    fn forget_session_removes_it_from_the_badge() {
        let (sink, n) = fixture();
        n.set_permission(NotifyPermission::Granted);
        n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_000,
        );
        assert_eq!(n.forget_session("s1"), None);
        assert_eq!(sink.dismissed(), vec!["s1".to_string()]);

        // 同じ session_id が再登録されたとき、prev_state / last_notified_at が
        // 消えていなければ SuppressNotTransition / SuppressRateLimited に誤判定される。
        let out = n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_100,
        );
        assert_eq!(out.decision, NotifyDecision::Post(NotifyKind::WaitingInput));
    }

    #[test]
    fn the_first_spawn_asks_for_permission_exactly_once() {
        let (_sink, n) = fixture();
        let first = n.on_state(
            &payload("s1", RuntimeState::Running, StateReason::Spawned),
            0,
        );
        let second = n.on_state(
            &payload("s2", RuntimeState::Running, StateReason::Spawned),
            10,
        );

        assert!(first.request_permission);
        assert!(!second.request_permission);
    }

    #[test]
    fn a_known_permission_never_asks_again() {
        let (_sink, n) = fixture();
        n.set_permission(NotifyPermission::Denied);
        let out = n.on_state(
            &payload("s1", RuntimeState::Running, StateReason::Spawned),
            0,
        );
        assert!(!out.request_permission);

        // permission が Unknown に戻っても、一度要求済み（permission_requested）なら
        // 再要求しない。set_permission(Denied) が permission_requested を立てるのが
        // この検査の要であり、その後 Unknown に戻すことで第 3 項（!permission_requested）
        // だけが唯一の抑止になる状況を作る（PR 18 単位レビュー I-1）。
        n.set_permission(NotifyPermission::Unknown);
        let out = n.on_state(
            &payload("s1", RuntimeState::Running, StateReason::Spawned),
            10,
        );
        assert!(!out.request_permission);
    }

    #[test]
    fn a_missing_label_falls_back_to_a_short_session_id() {
        let sink = Arc::new(RecordingSink::default());
        let labels: LabelResolver = Arc::new(|_id: &str| None);
        let n = Notifier::new(sink.clone() as Arc<dyn NotificationSink>, labels);
        n.set_permission(NotifyPermission::Granted);

        n.on_state(
            &payload(
                "3f2a9c1e-0000",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_000,
        );

        assert_eq!(sink.posted()[0].title, "入力待ち: セッション 3f2a9c1e");
    }

    /// 必須要件: `format_notification()` の戻り値 `(title, body)` を
    /// `NotificationRequest` に詰める箇所で title/body を取り違えていないかを検出する。
    ///
    /// `recording_sink_captures_posts_in_order`（sink_tests）は `NotificationRequest` を
    /// テスト自身が名前付きフィールドで組み立てており、`format_notification()` の戻り値を
    /// 詰め替えるコードパスを一切通らないため、この取り違えを構造的に観測できない。
    /// このテストは `Notifier::on_state` を実際に駆動し、`format_notification()` から
    /// `Notifier::on_state` 内の詰め替えコードを経由して `RecordingSink` に届いた値を見る。
    ///
    /// 期待値は手入力ではなく policy.rs の既存テスト
    /// `waiting_input_title_follows_the_design_doc`（title: "fix-login" の場合）から
    /// そのままコピーする。body の区切りは U+00B7 MIDDLE DOT（`·`）であり、
    /// 中黒 U+30FB（`・`）ではない。
    #[test]
    fn notifier_keeps_the_title_in_title_and_the_body_in_body() {
        let sink = Arc::new(RecordingSink::default());
        let labels: LabelResolver = Arc::new(|_id: &str| {
            Some(SessionLabel {
                title: "fix-login".into(),
                project_name: "kamux".into(),
                location: "session/fix-login".into(),
            })
        });
        let n = Notifier::new(sink.clone() as Arc<dyn NotificationSink>, labels);
        n.set_permission(NotifyPermission::Granted);

        n.on_state(
            &payload(
                "s1",
                RuntimeState::WaitingInput,
                StateReason::HookNotification,
            ),
            1_000,
        );

        let posted = sink.posted();
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].session_id, "s1");
        assert_eq!(posted[0].kind, NotifyKind::WaitingInput);
        assert_eq!(posted[0].title, "入力待ち: fix-login");
        assert_eq!(posted[0].body, "kamux · session/fix-login");
    }
}
