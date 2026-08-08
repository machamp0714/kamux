//! 通知の発火判定・バッジ数計算・文言生成。
//!
//! このモジュールは OS API に一切触れない。すべて純粋関数と純粋なデータで構成し、
//! `cargo test` で全分岐を検証できるようにする（設計書 §13 / 契約 §14）。

use serde::{Deserialize, Serialize};

use crate::model::{RuntimeState, StateReason};

/// 同一セッションに対する通知の最小間隔（ミリ秒）。
pub const NOTIFY_MIN_INTERVAL_MS: i64 = 10_000;

/// 通知の応答待ちタイムアウト（ミリ秒）。
///
/// `mac-usernotifications` は「通知センターで『すべて消去』を押されると
/// 応答 future が永久に解決しない」既知の落とし穴を持つため、
/// actionable な通知には必ずタイムアウトを設定する。
pub const NOTIFY_TIMEOUT_MS: u32 = 300_000;

/// 同時にクリック応答を待てる通知の上限。超えた分は「通知は出すが応答は待たない」。
pub const MAX_INFLIGHT_WAITERS: usize = 32;

/// Dock バッジを設定する対象ウィンドウのラベル（`tauri.conf.json` と一致させること）。
pub const MAIN_WINDOW_LABEL: &str = "main";

/// 通知の種類。設計書 §9.1 の 2 行に対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyKind {
    /// 「入力待ち: {title}」
    WaitingInput,
    /// 「応答完了: {title}」
    Stopped,
}

/// macOS の通知許可状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyPermission {
    /// まだ問い合わせていない / 判定不能。
    #[default]
    Unknown,
    Granted,
    Denied,
}

/// フロントの表示中ビュー。TS の `AppStore['view']` と 1:1。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    Kanban,
    Terminal,
    Editor,
}

/// 「今どのセッションがユーザーの目に入っているか」。
///
/// `view` / `paneAssignment` は Zustand ストア側にしか無いため、
/// フロントから `set_visibility_context` コマンドで push される。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VisibilityContext {
    /// メインウィンドウがフォーカスされているか（Rust が `WindowEvent::Focused` で追跡）。
    pub window_focused: bool,
    /// `None` はフロント未初期化。抑制判定には使わない（= 抑制しない）。
    pub view: Option<ViewKind>,
    /// 表示中ペインに割り当てられているセッション。
    pub visible_session_ids: Vec<String>,
}

/// 判定結果。`Post` 以外はすべて「なぜ出さなかったか」を保持する（デバッグとテスト用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyDecision {
    Post(NotifyKind),
    /// 通知対象の状態遷移ではない。
    SuppressIrrelevantState,
    /// 通知権限が拒否されている。
    SuppressPermissionDenied,
    /// 同じ状態への再入（エッジトリガでない）。
    SuppressNotTransition,
    /// 該当セッションが今まさに画面に出ている。
    SuppressVisible,
    /// 前回通知から `NOTIFY_MIN_INTERVAL_MS` 未満。
    SuppressRateLimited,
}

/// 通知文言の材料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLabel {
    /// `Session::title`。
    pub title: String,
    /// `Project::name`。
    pub project_name: String,
    /// `branch` があればブランチ名、無ければ「リポジトリ直上」。
    pub location: String,
}

/// 状態遷移が通知に値するかを判定する。
///
/// `StateReason::HookPermission`（`PermissionRequest` hook）も `HookNotification` と
/// 同じ扱いにする。契約 §12.4 のとおり、権限ダイアログの表示は「ユーザーの承認待ち」の
/// 最も直接的な信号だから。
///
/// `StateReason::StartupNormalize` は起動時の正規化なので絶対に通知しない
/// （契約 §2「`interrupted` を実行中に付けてはならない」と対になる規則）。
///
/// 契約 §8 の `StateReason` は 13 バリアントあるが、ここで拾うのは 5 つだけ。
/// 残りは網羅的に `None` に落ちる。
pub fn notify_kind_for(next: RuntimeState, reason: StateReason) -> Option<NotifyKind> {
    match (next, reason) {
        (RuntimeState::WaitingInput, StateReason::HookNotification)
        | (RuntimeState::WaitingInput, StateReason::HookPermission)
        | (RuntimeState::WaitingInput, StateReason::BelDetected) => Some(NotifyKind::WaitingInput),
        (RuntimeState::Idle, StateReason::HookStop)
        | (RuntimeState::Idle, StateReason::SilenceTimeout) => Some(NotifyKind::Stopped),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RuntimeState, StateReason};

    #[test]
    fn hook_notification_into_waiting_input_is_a_waiting_notification() {
        assert_eq!(
            notify_kind_for(RuntimeState::WaitingInput, StateReason::HookNotification),
            Some(NotifyKind::WaitingInput)
        );
    }

    #[test]
    fn permission_request_hook_is_also_a_waiting_notification() {
        // 契約 §12.4: PermissionRequest は「ユーザーの承認待ち」の最も直接的な信号
        assert_eq!(
            notify_kind_for(RuntimeState::WaitingInput, StateReason::HookPermission),
            Some(NotifyKind::WaitingInput)
        );
    }

    #[test]
    fn bel_detected_into_waiting_input_is_a_waiting_notification() {
        assert_eq!(
            notify_kind_for(RuntimeState::WaitingInput, StateReason::BelDetected),
            Some(NotifyKind::WaitingInput)
        );
    }

    #[test]
    fn hook_stop_into_idle_is_a_stopped_notification() {
        assert_eq!(
            notify_kind_for(RuntimeState::Idle, StateReason::HookStop),
            Some(NotifyKind::Stopped)
        );
    }

    #[test]
    fn silence_timeout_into_idle_is_a_stopped_notification() {
        assert_eq!(
            notify_kind_for(RuntimeState::Idle, StateReason::SilenceTimeout),
            Some(NotifyKind::Stopped)
        );
    }

    #[test]
    fn spawned_running_and_exited_do_not_notify() {
        assert_eq!(
            notify_kind_for(RuntimeState::Running, StateReason::Spawned),
            None
        );
        assert_eq!(
            notify_kind_for(RuntimeState::Exited, StateReason::PtyExited),
            None
        );
        assert_eq!(
            notify_kind_for(RuntimeState::Idle, StateReason::UserStopped),
            None
        );
    }

    #[test]
    fn output_activity_and_user_input_do_not_notify() {
        // 🟡 の解除（UserInput）と 🟢 への復帰（OutputActivity）は「気づくべきこと」ではない
        assert_eq!(
            notify_kind_for(RuntimeState::Running, StateReason::OutputActivity),
            None
        );
        assert_eq!(
            notify_kind_for(RuntimeState::Running, StateReason::UserInput),
            None
        );
    }

    #[test]
    fn resume_failure_does_not_notify() {
        // resume 失敗は設計書 §12 のとおりトーストで stderr を出す領分。通知にはしない
        assert_eq!(
            notify_kind_for(RuntimeState::Exited, StateReason::ResumeFailed),
            None
        );
    }

    #[test]
    fn startup_normalize_never_notifies() {
        assert_eq!(
            notify_kind_for(RuntimeState::Interrupted, StateReason::StartupNormalize),
            None
        );
    }

    #[test]
    fn spawn_failure_into_error_does_not_notify() {
        // StateReason は契約 §8 の 13 値。SpawnFailed（error 状態への遷移）も通知しない。
        // これで notify_kind_for が拾う 5 値 / 拾わない 8 値の網羅が宣言として閉じる
        assert_eq!(
            notify_kind_for(RuntimeState::Error, StateReason::SpawnFailed),
            None
        );
    }

    #[test]
    fn default_permission_is_unknown() {
        assert_eq!(NotifyPermission::default(), NotifyPermission::Unknown);
    }

    #[test]
    fn default_visibility_has_no_view() {
        let v = VisibilityContext::default();
        assert!(!v.window_focused);
        assert_eq!(v.view, None);
        assert!(v.visible_session_ids.is_empty());
    }

    #[test]
    fn notify_constants_match_the_contract() {
        // 契約 §83.4（00-contracts.md）が正典。値を変えるときは契約を先に直すこと
        assert_eq!(NOTIFY_MIN_INTERVAL_MS, 10_000);
        assert_eq!(NOTIFY_TIMEOUT_MS, 300_000);
        assert_eq!(MAX_INFLIGHT_WAITERS, 32);
        assert_eq!(MAIN_WINDOW_LABEL, "main");
    }

    /// `Deserialize` を持つので `serde_json` での roundtrip を直接書く。
    /// 契約 §83.3（00-contracts.md）が正典。TS 側は `src/types/model.ts` の
    /// `export type NotifyPermission = 'unknown' | 'granted' | 'denied';`。
    #[test]
    fn notify_permission_matches_contract_strings() {
        let cases: [(NotifyPermission, &str); 3] = [
            (NotifyPermission::Unknown, "unknown"),
            (NotifyPermission::Granted, "granted"),
            (NotifyPermission::Denied, "denied"),
        ];
        for (value, expected) in cases {
            let json = serde_json::to_string(&value).expect("serialize");
            assert_eq!(
                json,
                format!("\"{expected}\""),
                "NotifyPermission の serde 表現が契約 §83.3 と違う: {value:?}"
            );
            let back: NotifyPermission = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, value);
        }
    }

    /// 設計書 §9.1 の 2 行（「入力待ち」「応答完了」）に対応する wire 文字列を固定する。
    #[test]
    fn notify_kind_matches_contract_strings() {
        let cases: [(NotifyKind, &str); 2] = [
            (NotifyKind::WaitingInput, "waiting_input"),
            (NotifyKind::Stopped, "stopped"),
        ];
        for (value, expected) in cases {
            let json = serde_json::to_string(&value).expect("serialize");
            assert_eq!(
                json,
                format!("\"{expected}\""),
                "NotifyKind の serde 表現が違う: {value:?}"
            );
            let back: NotifyKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, value);
        }
    }

    /// 契約 §7.2（00-contracts.md）が正典。`pub enum ViewKind { Kanban, Terminal, Editor }`。
    #[test]
    fn view_kind_matches_contract_strings() {
        let cases: [(ViewKind, &str); 3] = [
            (ViewKind::Kanban, "kanban"),
            (ViewKind::Terminal, "terminal"),
            (ViewKind::Editor, "editor"),
        ];
        for (value, expected) in cases {
            let json = serde_json::to_string(&value).expect("serialize");
            assert_eq!(
                json,
                format!("\"{expected}\""),
                "ViewKind の serde 表現が契約 §7.2 と違う: {value:?}"
            );
            let back: ViewKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, value);
        }
    }
}
