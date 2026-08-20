//! macOS の `UNUserNotificationCenter` へ通知を出す [`NotificationSink`] 実装。
//!
//! `mac-usernotifications` の async API を、通知ごとに立てた専用スレッドの上で
//! `block_on` して駆動する。
//!
//! `Notification::send_blocking`（メソッド版。`notification.rs:280`）は使わない
//! —— `block_on_current` を通り（`notification.rs:283`）、メインの run loop が
//! 回っていない状況で失敗する（契約 §106.5 の理由 2）。ここで使うのは async 版の
//! `Notification::send`（`notification.rs:268`）であり、内部で呼ぶ
//! `send_and_wait_for_delivery` を素の `await` で駆動するだけで `block_on_current`
//! を通らない。応答待ちの `NotificationHandle::response`（`send.rs:91`）も同様に
//! 素の `await` である。この crate に `wait_for_response` という名前の関数は無い
//! （`notify-rust` 側の API 名であり、混同しない）。ここはメインスレッドではないので、
//! 素の `block_on`（`futures_lite::future::block_on` の re-export）でこれらの
//! `Future` を駆動する。応答の完了ハンドラは Apple 側のキューとメインスレッドで走り、
//! Tauri が run loop を回し続けているので、この待機は成立する。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mac_usernotifications::{block_on, AuthorizationStatus, Notification};

use super::policy::{NotifyPermission, MAX_INFLIGHT_WAITERS, NOTIFY_TIMEOUT_MS};
use super::{ClickHandler, NotificationRequest, NotificationSink};

/// セッションごとに固定の通知識別子。同じ id で再送すると macOS が通知を置き換える。
pub fn notification_id_string(session_id: &str) -> String {
    format!("kamux-session-{session_id}")
}

/// macOS の認可状態を kamux の 3 値に写像する。
///
/// `Provisional` / `Ephemeral` は「静かに配信される」状態で、通知そのものは届くため
/// `Granted` 扱いにする。`NotDetermined` はまだ聞いていないので `Unknown`。
pub fn map_authorization(status: AuthorizationStatus) -> NotifyPermission {
    match status {
        AuthorizationStatus::Authorized
        | AuthorizationStatus::Provisional
        | AuthorizationStatus::Ephemeral => NotifyPermission::Granted,
        AuthorizationStatus::Denied => NotifyPermission::Denied,
        AuthorizationStatus::NotDetermined | AuthorizationStatus::Unknown => {
            NotifyPermission::Unknown
        }
    }
}

/// 1 通分のバナーの中身。[`NotificationRequest`] から機械的に導く。
///
/// `post` のうち送信・応答待ち・クリック配線（`Notification::send` /
/// `NotificationHandle::response` 呼び出し以降）は OS を叩くので単体テストに
/// 乗らない。それより手前の詰め替え（本関数と [`build_notification`]）は
/// 純関数として切り出してあり単体テストで観測できる。`title` と `body` はどちらも
/// `String` で、詰め替えで入れ替わっても型検査を通る（契約 §81.2 の 2 条件）。
/// この構造体を組み立てる詰め替えが赤くなる観測点は
/// `payload_keeps_title_and_body_distinct` である。この後さらに
/// `mac_usernotifications::Notification` の builder（`.title()`/`.message()`）
/// へ渡す詰め替えが続くが、それは別の関数 [`build_notification`] が持ち、
/// `build_notification_keeps_title_and_body_distinct_in_the_builder` が守る
/// （タスク単位レビュー I-1。詰め替えは 2 段あり、観測点も 2 つに分かれている）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPayload {
    pub id: String,
    pub title: String,
    pub message: String,
    pub timeout: Duration,
}

pub fn build_payload(req: &NotificationRequest) -> NotificationPayload {
    NotificationPayload {
        id: notification_id_string(&req.session_id),
        title: req.title.clone(),
        message: req.body.clone(),
        // 契約 §106.5.2: `as` を書かない。`u32` -> `u64` は `From` が通る。
        timeout: Duration::from_millis(NOTIFY_TIMEOUT_MS.into()),
    }
}

/// [`NotificationPayload`] を `mac_usernotifications::Notification` の builder
/// へ配線する。`build_payload` を切り出しただけでは `post` の中のこの詰め替えが
/// 無検査のまま残る（タスク単位レビュー I-1 / I-3）ため、ここへさらに切り出し、
/// `Notification` の `Debug` 出力（`#[derive(Debug, Default)]`。
/// `notification.rs:23`）で `title`/`body` の取り違えを観測できるようにする。
fn build_notification(payload: &NotificationPayload) -> Notification {
    // `Notification` のビルダは self を消費して Self を返す形である
    // （`notification.rs:59-191`）。`let mut n = ...; n.a().b();` とは書けない。
    Notification::new()
        .title(&payload.title)
        .message(&payload.message)
        .id(&payload.id)
        .timeout(payload.timeout)
}

/// プロンプトを出さずに現在の認可状態を読む。
///
/// `get_notification_settings_blocking` は `futures_lite::future::block_on` を
/// 使う（`block_on_current` ではない）ので、どのスレッドから呼んでもよい。
pub fn read_permission() -> NotifyPermission {
    match mac_usernotifications::blocking::get_notification_settings() {
        Ok(s) => map_authorization(s.authorization_status),
        Err(e) => {
            tracing::warn!("failed to read notification settings: {e}");
            NotifyPermission::Unknown
        }
    }
}

/// 通知許可のプロンプトを出す。既に拒否済みなら macOS は即座に false を返す。
pub async fn request_permission() -> NotifyPermission {
    match mac_usernotifications::request_auth().await {
        Ok(true) => NotifyPermission::Granted,
        Ok(false) => NotifyPermission::Denied,
        Err(e) => {
            tracing::warn!("request_auth failed: {e}");
            NotifyPermission::Unknown
        }
    }
}

/// 同時にクリック応答を待てる枠を表す RAII ガード。
///
/// [`acquire`](Self::acquire) が枠を取れたら `Some(Self)` を返し、上限
/// （[`MAX_INFLIGHT_WAITERS`]）に達していれば `None` を返す。減算は
/// `acquire` の拒否経路（枠を取れなかったときの巻き戻し）と `Drop` の
/// 2 箇所にある（タスク単位レビュー I-2 / I-5。旧実装は `AtomicUsize` の
/// 増減を早期 return の各経路で手で書いており、`fetch_sub` を書き忘れると
/// 枠が永久に漏れ、`MAX_INFLIGHT_WAITERS` 回でクリック経路が恒久的に
/// 失われた）。`Drop` 側は `dropping_a_guard_frees_the_slot_for_reacquisition`
/// が守る（実測: `Drop::drop` の中の `fetch_sub` を消す変異はコンパイルが
/// 通り、当該テストが赤になることを確認した）。`acquire` の拒否経路側は
/// `inflight_guard_rejects_the_slot_past_the_limit` の
/// `assert_eq!(counter.load(...), MAX_INFLIGHT_WAITERS)` が守る。
struct InflightGuard {
    inflight: Arc<AtomicUsize>,
}

impl InflightGuard {
    /// 枠を取得する。上限に達していれば `None`。
    fn acquire(inflight: &Arc<AtomicUsize>) -> Option<Self> {
        if inflight.fetch_add(1, Ordering::SeqCst) >= MAX_INFLIGHT_WAITERS {
            inflight.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self {
            inflight: Arc::clone(inflight),
        })
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 本番用の [`NotificationSink`]。
pub struct MacNotificationSink {
    on_click: ClickHandler,
    /// クリック応答を待っているスレッド数。上限を超えたら待機せず通知だけ出す。
    inflight: Arc<AtomicUsize>,
}

impl MacNotificationSink {
    pub fn new(on_click: ClickHandler) -> Self {
        if let Err(e) = mac_usernotifications::check_bundle() {
            tracing::warn!("no bundle identifier; notifications will not be delivered: {e}");
        }
        Self {
            on_click,
            inflight: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl NotificationSink for MacNotificationSink {
    fn post(&self, req: NotificationRequest) {
        let on_click = Arc::clone(&self.on_click);
        let inflight = Arc::clone(&self.inflight);
        let payload = build_payload(&req);
        let session_id = req.session_id.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("kamux-notify-{session_id}"))
            .spawn(move || {
                block_on(async move {
                    let handle = match build_notification(&payload).send().await {
                        Ok(h) => h,
                        Err(e) => {
                            tracing::warn!("failed to show notification: {e}");
                            return;
                        }
                    };

                    // 待機枠が上限に達していたら、通知は出したまま応答待ちを諦める。
                    // ハンドルを drop してもバナーは表示されたままで、
                    // クリックの経路だけが失われる。`post` の中で減算を
                    // 個別に書く必要はない（`InflightGuard` の `Drop` が行う）。
                    let guard = match InflightGuard::acquire(&inflight) {
                        Some(g) => g,
                        None => {
                            tracing::warn!(
                                "too many pending notifications; click routing disabled for this one"
                            );
                            return;
                        }
                    };

                    // `response()` は handle を消費する（`send.rs:91`）。
                    let result = handle.response().await;
                    drop(guard);
                    match result {
                        Ok(resp) if resp.is_default_action() => on_click(&session_id),
                        Ok(_) => {}
                        Err(e) => tracing::debug!("notification response wait ended: {e}"),
                    }
                });
            });

        if let Err(e) = spawned {
            tracing::warn!("failed to spawn notification thread: {e}");
        }
    }

    fn dismiss(&self, session_id: &str) {
        // `close_delivered_blocking`（= `blocking::close_delivered`）は worker
        // スレッドへ dispatch して即座に返る fire-and-forget であり、呼び出し元を
        // ブロックしない（`send.rs:205-213` の doc と実装）。したがって M2-1 の
        // consumer スレッドから直接呼んでよく、退避用のスレッドは要らない。
        mac_usernotifications::blocking::close_delivered(&notification_id_string(session_id));
    }
}

#[cfg(test)]
mod tests {
    use super::super::NotifyKind;
    use super::*;

    #[test]
    fn notification_id_is_stable_per_session() {
        let a = notification_id_string("3f2a9c1e");
        let b = notification_id_string("3f2a9c1e");
        assert_eq!(a, b);
        assert_eq!(a, "kamux-session-3f2a9c1e");
    }

    #[test]
    fn different_sessions_get_different_notification_ids() {
        assert_ne!(notification_id_string("s1"), notification_id_string("s2"));
    }

    #[test]
    fn authorized_maps_to_granted() {
        use mac_usernotifications::AuthorizationStatus as A;
        assert_eq!(map_authorization(A::Authorized), NotifyPermission::Granted);
        assert_eq!(map_authorization(A::Provisional), NotifyPermission::Granted);
        assert_eq!(map_authorization(A::Ephemeral), NotifyPermission::Granted);
    }

    #[test]
    fn denied_maps_to_denied() {
        use mac_usernotifications::AuthorizationStatus as A;
        assert_eq!(map_authorization(A::Denied), NotifyPermission::Denied);
    }

    #[test]
    fn undecided_maps_to_unknown() {
        use mac_usernotifications::AuthorizationStatus as A;
        assert_eq!(
            map_authorization(A::NotDetermined),
            NotifyPermission::Unknown
        );
        assert_eq!(map_authorization(A::Unknown), NotifyPermission::Unknown);
    }

    fn req(session_id: &str, title: &str, body: &str) -> NotificationRequest {
        NotificationRequest {
            session_id: session_id.to_string(),
            kind: NotifyKind::WaitingInput,
            title: title.to_string(),
            body: body.to_string(),
        }
    }

    /// `post` のうち送信・応答待ち・クリック配線は OS を叩くので単体テストに
    /// 乗らないが、詰め替え自体は純関数として切り出してあり観測できる。
    /// `title` と `body` はどちらも `String` で、詰め替えで入れ替わっても
    /// 型検査を通る（契約 §81.2 の 2 条件）。`build_payload` の中の入れ替えが
    /// 赤くなる観測点はここだけである。`build_notification`（`NotificationPayload`
    /// から builder への配線）の入れ替えは
    /// `build_notification_keeps_title_and_body_distinct_in_the_builder`
    /// が別途守る。
    #[test]
    fn payload_keeps_title_and_body_distinct() {
        let p = build_payload(&req("3f2a9c1e", "入力待ち", "kamux · feat/x"));
        assert_eq!(p.title, "入力待ち");
        assert_eq!(p.message, "kamux · feat/x");
        assert_eq!(p.id, "kamux-session-3f2a9c1e");
    }

    /// 契約 §106.5.2: 値も型も動かさず `Duration::from_millis(NOTIFY_TIMEOUT_MS.into())`。
    #[test]
    fn payload_timeout_comes_from_the_policy_constant() {
        let p = build_payload(&req("s1", "t", "b"));
        assert_eq!(p.timeout, std::time::Duration::from_millis(300_000));
        assert_eq!(p.timeout.as_millis() as u32, NOTIFY_TIMEOUT_MS);
    }

    /// タスク単位レビュー I-2: `MAX_INFLIGHT_WAITERS` 個までは枠を確保できる。
    #[test]
    fn inflight_guard_allows_up_to_the_limit() {
        let counter = Arc::new(AtomicUsize::new(0));
        let guards: Vec<Option<InflightGuard>> = (0..MAX_INFLIGHT_WAITERS)
            .map(|_| InflightGuard::acquire(&counter))
            .collect();
        assert!(guards.iter().all(Option::is_some));
    }

    /// タスク単位レビュー I-2 / 自A: 上限ちょうどまでは確保できることも合わせて
    /// 確認する。そうしないと「1 本目から拒否される」実装（`>=
    /// MAX_INFLIGHT_WAITERS` を `>= 1` にする変異）でも本テストの後半だけは
    /// 緑になってしまう —— 上限より 1 本多いと必ず `None` が返るのは、
    /// 閾値が 1 でも 32 でも同じ結果になるため、値そのものを区別できない。
    #[test]
    fn inflight_guard_rejects_the_slot_past_the_limit() {
        let counter = Arc::new(AtomicUsize::new(0));
        let guards: Vec<Option<InflightGuard>> = (0..MAX_INFLIGHT_WAITERS)
            .map(|_| InflightGuard::acquire(&counter))
            .collect();
        assert!(
            guards.iter().all(Option::is_some),
            "expected all {MAX_INFLIGHT_WAITERS} slots to be acquired"
        );
        let rejected = InflightGuard::acquire(&counter);
        assert!(rejected.is_none());
        assert_eq!(counter.load(Ordering::SeqCst), MAX_INFLIGHT_WAITERS);
    }

    /// タスク単位レビュー I-2 / 自B: ガードを drop すると枠が戻り、再び
    /// acquire できる。旧実装は早期 return 経路で `fetch_sub` を書き忘れると
    /// 枠が永久に漏れる欠陥を持っていた。この `Drop::drop` の中身を壊す変異は
    /// 本テストが検出する。
    #[test]
    fn dropping_a_guard_frees_the_slot_for_reacquisition() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut guards: Vec<Option<InflightGuard>> = (0..MAX_INFLIGHT_WAITERS)
            .map(|_| InflightGuard::acquire(&counter))
            .collect();
        assert!(guards.iter().all(Option::is_some));

        guards.pop();
        let reacquired = InflightGuard::acquire(&counter);
        assert!(reacquired.is_some());
    }

    /// タスク単位レビュー I-3 / 自E2 / I-6: `post` が `Notification` へ渡す
    /// `title`/`message`/`id`/`timeout` は `build_notification` のここでしか
    /// 観測できない。`NotificationPayload` の 4 フィールドを見る。`Notification`
    /// の残り 9 フィールドは `Default` のままで射程外。
    /// `Notification` の `Debug` はフィールド名付きで出力されるので
    /// （`notification.rs:24-56` の `#[derive(Debug, Default)]`）、値だけの
    /// `contains` ではなく `title: "…"` / `body: "…"` の対で見る —— 値だけを
    /// 見ると入れ替えても両方の文字列が出力に在るため常に緑になる。`id`/`timeout`
    /// も同様にフィールド名付きで見る（`Debug` 上のフィールド名は builder の
    /// メソッド名と異なり、`notification_id`/`action_timeout` である）。
    #[test]
    fn build_notification_keeps_title_and_body_distinct_in_the_builder() {
        let p = build_payload(&req("3f2a9c1e", "入力待ち", "kamux · feat/x"));
        let debug = format!("{:?}", build_notification(&p));
        assert!(
            debug.contains(r#"title: "入力待ち""#),
            "debug output was: {debug}"
        );
        assert!(
            debug.contains(r#"body: "kamux · feat/x""#),
            "debug output was: {debug}"
        );
        assert!(
            debug.contains(r#"notification_id: Some("kamux-session-3f2a9c1e")"#),
            "debug output was: {debug}"
        );
        assert!(
            debug.contains("action_timeout: Some(300s)"),
            "debug output was: {debug}"
        );
    }
}
