//! 通知（macOS 通知・Dock バッジ）関連モジュール。
//!
//! `policy` サブモジュールが判定ロジックの型と純粋関数を持つ。
//! OS API に触れる実装は後続タスクで別サブモジュールとして追加する。

pub mod policy;

use std::sync::{Arc, Mutex};

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
