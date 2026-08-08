//! 通知の発火判定・バッジ数計算・文言生成。
//!
//! このモジュールは OS API に一切触れない。すべて純粋関数と純粋なデータで構成し、
//! `cargo test` で全分岐を検証できるようにする（設計書 §13 / 契約 §14）。

use std::collections::BTreeMap;

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

/// 「そのセッションを今ユーザーが見ているか」。
///
/// ウィンドウがフォーカスされ、ターミナル画面が表示され、かつ表示中ペインに
/// 割り当てられているときだけ true。エディタ画面は nvim であり agent の出力は
/// 見えていないので、抑制対象にしない。
pub fn is_session_visible(session_id: &str, v: &VisibilityContext) -> bool {
    v.window_focused
        && matches!(v.view, Some(ViewKind::Terminal))
        && v.visible_session_ids.iter().any(|id| id == session_id)
}

/// `decide` の入力。引数が多いので構造体でまとめる。
#[derive(Debug, Clone)]
pub struct DecisionInput<'a> {
    pub session_id: &'a str,
    /// 直前の `runtime_state`。初観測なら `None`。
    pub prev: Option<RuntimeState>,
    pub next: RuntimeState,
    pub reason: StateReason,
    pub permission: NotifyPermission,
    pub visibility: &'a VisibilityContext,
    /// 同一セッションに前回通知を出した時刻（Unix epoch ミリ秒）。
    pub last_notified_at_ms: Option<i64>,
    pub now_ms: i64,
}

/// 通知を出すか、出さないならなぜかを決める。
///
/// 判定順は「対象状態か → 権限 → エッジトリガ → 表示中 → レート制限」。
/// 対象外の状態遷移を最初に弾くのは、権限が拒否されていても
/// 「そもそも通知の対象ではない」ほうが原因として正確だから。
pub fn decide(input: &DecisionInput<'_>) -> NotifyDecision {
    let Some(kind) = notify_kind_for(input.next, input.reason) else {
        return NotifyDecision::SuppressIrrelevantState;
    };
    if input.permission == NotifyPermission::Denied {
        return NotifyDecision::SuppressPermissionDenied;
    }
    if input.prev == Some(input.next) {
        return NotifyDecision::SuppressNotTransition;
    }
    if is_session_visible(input.session_id, input.visibility) {
        return NotifyDecision::SuppressVisible;
    }
    if let Some(last) = input.last_notified_at_ms {
        // 時計が巻き戻ると差は負になり、NOTIFY_MIN_INTERVAL_MS 未満なので抑制側に倒れる。
        // saturating_sub は i64::MIN 方向のオーバーフロー panic を避けるためであり、
        // 0 にクランプするわけではない（負の値のまま比較に使われる）。
        if input.now_ms.saturating_sub(last) < NOTIFY_MIN_INTERVAL_MS {
            return NotifyDecision::SuppressRateLimited;
        }
    }
    NotifyDecision::Post(kind)
}

/// リポジトリ直上モードのときに通知本文へ出す文言。
pub const LOCATION_IN_PLACE: &str = "リポジトリ直上";

impl SessionLabel {
    /// セッションが DB から引けなかったときの退避ラベル。
    /// 通知が完全に沈黙するより、識別子だけでも出したほうがましなので用意する。
    pub fn fallback(session_id: &str) -> Self {
        let short: String = session_id.chars().take(8).collect();
        Self {
            title: format!("セッション {short}"),
            project_name: "kamux".to_string(),
            location: LOCATION_IN_PLACE.to_string(),
        }
    }
}

/// Dock バッジに出す「要対応セッション数」。
///
/// `waiting_input` のみを数える。`exited` はアーカイブされるまで DB に残り続けるため、
/// 含めるとバッジが恒常的に非ゼロになり「今すぐ対応すべき件数」の意味を失う。
/// 0 件のときは `None` を返し、呼び出し側が `set_badge_count(None)` でバッジを消す。
pub fn badge_count(states: &BTreeMap<String, RuntimeState>) -> Option<i64> {
    let n = states
        .values()
        .filter(|s| **s == RuntimeState::WaitingInput)
        .count() as i64;
    if n == 0 {
        None
    } else {
        Some(n)
    }
}

/// 通知のタイトルと本文を組み立てる。タイトルは設計書 §9.1 の表に逐語で従う。
pub fn format_notification(kind: NotifyKind, label: &SessionLabel) -> (String, String) {
    let title = match kind {
        NotifyKind::WaitingInput => format!("入力待ち: {}", label.title),
        NotifyKind::Stopped => format!("応答完了: {}", label.title),
    };
    let body = format!("{} · {}", label.project_name, label.location);
    (title, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RuntimeState, StateReason};

    fn ctx(focused: bool, view: Option<ViewKind>, ids: &[&str]) -> VisibilityContext {
        VisibilityContext {
            window_focused: focused,
            view,
            visible_session_ids: ids.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn input<'a>(
        prev: Option<RuntimeState>,
        next: RuntimeState,
        reason: StateReason,
        permission: NotifyPermission,
        visibility: &'a VisibilityContext,
        last: Option<i64>,
        now_ms: i64,
    ) -> DecisionInput<'a> {
        DecisionInput {
            session_id: "s1",
            prev,
            next,
            reason,
            permission,
            visibility,
            last_notified_at_ms: last,
            now_ms,
        }
    }

    #[test]
    fn posts_on_first_transition_into_waiting_input() {
        let v = ctx(false, Some(ViewKind::Kanban), &[]);
        let i = input(
            Some(RuntimeState::Running),
            RuntimeState::WaitingInput,
            StateReason::HookNotification,
            NotifyPermission::Granted,
            &v,
            None,
            1_000,
        );
        assert_eq!(decide(&i), NotifyDecision::Post(NotifyKind::WaitingInput));
    }

    #[test]
    fn posts_when_permission_is_still_unknown() {
        let v = ctx(false, None, &[]);
        let i = input(
            Some(RuntimeState::Running),
            RuntimeState::Idle,
            StateReason::HookStop,
            NotifyPermission::Unknown,
            &v,
            None,
            1_000,
        );
        assert_eq!(decide(&i), NotifyDecision::Post(NotifyKind::Stopped));
    }

    #[test]
    fn suppresses_when_permission_is_denied() {
        let v = ctx(false, None, &[]);
        let i = input(
            Some(RuntimeState::Running),
            RuntimeState::WaitingInput,
            StateReason::HookNotification,
            NotifyPermission::Denied,
            &v,
            None,
            1_000,
        );
        assert_eq!(decide(&i), NotifyDecision::SuppressPermissionDenied);
    }

    #[test]
    fn suppresses_irrelevant_state_before_anything_else() {
        let v = ctx(true, Some(ViewKind::Terminal), &["s1"]);
        let i = input(
            Some(RuntimeState::Running),
            RuntimeState::Exited,
            StateReason::PtyExited,
            NotifyPermission::Denied,
            &v,
            None,
            1_000,
        );
        assert_eq!(decide(&i), NotifyDecision::SuppressIrrelevantState);
    }

    #[test]
    fn suppresses_re_entry_into_the_same_state() {
        let v = ctx(false, Some(ViewKind::Kanban), &[]);
        let i = input(
            Some(RuntimeState::WaitingInput),
            RuntimeState::WaitingInput,
            StateReason::HookNotification,
            NotifyPermission::Granted,
            &v,
            None,
            1_000,
        );
        assert_eq!(decide(&i), NotifyDecision::SuppressNotTransition);
    }

    #[test]
    fn posts_when_there_is_no_previous_state() {
        let v = ctx(false, Some(ViewKind::Kanban), &[]);
        let i = input(
            None,
            RuntimeState::WaitingInput,
            StateReason::HookNotification,
            NotifyPermission::Granted,
            &v,
            None,
            1_000,
        );
        assert_eq!(decide(&i), NotifyDecision::Post(NotifyKind::WaitingInput));
    }

    #[test]
    fn suppresses_when_the_session_is_on_screen() {
        let v = ctx(true, Some(ViewKind::Terminal), &["s1"]);
        let i = input(
            Some(RuntimeState::Running),
            RuntimeState::WaitingInput,
            StateReason::HookNotification,
            NotifyPermission::Granted,
            &v,
            None,
            1_000,
        );
        assert_eq!(decide(&i), NotifyDecision::SuppressVisible);
    }

    #[test]
    fn suppresses_within_the_minimum_interval() {
        let v = ctx(false, Some(ViewKind::Kanban), &[]);
        let i = input(
            Some(RuntimeState::Running),
            RuntimeState::WaitingInput,
            StateReason::HookNotification,
            NotifyPermission::Granted,
            &v,
            Some(1_000),
            1_000 + NOTIFY_MIN_INTERVAL_MS - 1,
        );
        assert_eq!(decide(&i), NotifyDecision::SuppressRateLimited);
    }

    #[test]
    fn posts_exactly_at_the_minimum_interval() {
        let v = ctx(false, Some(ViewKind::Kanban), &[]);
        let i = input(
            Some(RuntimeState::Running),
            RuntimeState::WaitingInput,
            StateReason::HookNotification,
            NotifyPermission::Granted,
            &v,
            Some(1_000),
            1_000 + NOTIFY_MIN_INTERVAL_MS,
        );
        assert_eq!(decide(&i), NotifyDecision::Post(NotifyKind::WaitingInput));
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_panic() {
        let v = ctx(false, Some(ViewKind::Kanban), &[]);
        let i = input(
            Some(RuntimeState::Running),
            RuntimeState::WaitingInput,
            StateReason::HookNotification,
            NotifyPermission::Granted,
            &v,
            Some(9_000),
            1_000,
        );
        assert_eq!(decide(&i), NotifyDecision::SuppressRateLimited);
    }

    #[test]
    fn permission_denied_wins_over_edge_trigger_visibility_and_rate_limit() {
        // 判定順序の連鎖を閉じる: 権限拒否以外の全ゲート（エッジトリガでない・表示中・
        // レート制限内）も同時に成立する入力で、権限が最優先で効くことを固定する。
        let v = ctx(true, Some(ViewKind::Terminal), &["s1"]);
        let i = input(
            Some(RuntimeState::WaitingInput),
            RuntimeState::WaitingInput,
            StateReason::HookNotification,
            NotifyPermission::Denied,
            &v,
            Some(1_000),
            1_000 + NOTIFY_MIN_INTERVAL_MS - 1,
        );
        assert_eq!(decide(&i), NotifyDecision::SuppressPermissionDenied);
    }

    #[test]
    fn not_a_transition_wins_over_visibility_and_rate_limit() {
        // エッジトリガでない（同一状態への再入）が、表示中・レート制限内より先に効く。
        let v = ctx(true, Some(ViewKind::Terminal), &["s1"]);
        let i = input(
            Some(RuntimeState::WaitingInput),
            RuntimeState::WaitingInput,
            StateReason::HookNotification,
            NotifyPermission::Granted,
            &v,
            Some(1_000),
            1_000 + NOTIFY_MIN_INTERVAL_MS - 1,
        );
        assert_eq!(decide(&i), NotifyDecision::SuppressNotTransition);
    }

    #[test]
    fn visibility_wins_over_rate_limit() {
        // 表示中が、レート制限内より先に効く。
        let v = ctx(true, Some(ViewKind::Terminal), &["s1"]);
        let i = input(
            Some(RuntimeState::Running),
            RuntimeState::WaitingInput,
            StateReason::HookNotification,
            NotifyPermission::Granted,
            &v,
            Some(1_000),
            1_000 + NOTIFY_MIN_INTERVAL_MS - 1,
        );
        assert_eq!(decide(&i), NotifyDecision::SuppressVisible);
    }

    #[test]
    fn visible_when_focused_on_terminal_and_assigned_to_a_pane() {
        assert!(is_session_visible(
            "s1",
            &ctx(true, Some(ViewKind::Terminal), &["s1"])
        ));
    }

    #[test]
    fn visible_when_assigned_to_the_second_pane() {
        assert!(is_session_visible(
            "s2",
            &ctx(true, Some(ViewKind::Terminal), &["s1", "s2"])
        ));
    }

    #[test]
    fn not_visible_when_window_is_in_background() {
        assert!(!is_session_visible(
            "s1",
            &ctx(false, Some(ViewKind::Terminal), &["s1"])
        ));
    }

    #[test]
    fn not_visible_on_kanban_even_if_focused() {
        assert!(!is_session_visible(
            "s1",
            &ctx(true, Some(ViewKind::Kanban), &["s1"])
        ));
    }

    #[test]
    fn not_visible_on_editor_because_agent_output_is_hidden() {
        assert!(!is_session_visible(
            "s1",
            &ctx(true, Some(ViewKind::Editor), &["s1"])
        ));
    }

    #[test]
    fn not_visible_when_another_session_occupies_the_pane() {
        assert!(!is_session_visible(
            "s1",
            &ctx(true, Some(ViewKind::Terminal), &["s9"])
        ));
    }

    #[test]
    fn not_visible_when_frontend_has_not_reported_yet() {
        assert!(!is_session_visible("s1", &ctx(true, None, &["s1"])));
    }

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

    fn states(pairs: &[(&str, RuntimeState)]) -> BTreeMap<String, RuntimeState> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn badge_is_none_when_nothing_needs_attention() {
        assert_eq!(badge_count(&states(&[])), None);
        assert_eq!(
            badge_count(&states(&[
                ("a", RuntimeState::Running),
                ("b", RuntimeState::Idle)
            ])),
            None
        );
    }

    #[test]
    fn badge_counts_only_waiting_input() {
        let s = states(&[
            ("a", RuntimeState::WaitingInput),
            ("b", RuntimeState::WaitingInput),
            ("c", RuntimeState::Running),
            ("d", RuntimeState::Idle),
        ]);
        assert_eq!(badge_count(&s), Some(2));
    }

    #[test]
    fn exited_sessions_do_not_inflate_the_badge() {
        // exited はアーカイブされるまで DB に残るため、数えるとバッジが恒常的に非ゼロになる
        let s = states(&[
            ("a", RuntimeState::Exited),
            ("b", RuntimeState::Exited),
            ("c", RuntimeState::Interrupted),
        ]);
        assert_eq!(badge_count(&s), None);
    }

    #[test]
    fn badge_counts_waiting_input_among_all_six_runtime_states() {
        // RuntimeState の 6 値すべてを置く。1 値でも欠けると、その値をフィルタに足す変異が
        // 素通りする（レビュー指摘: WaitingInput | Error でも既存フィクスチャは全緑だった）。
        let s = states(&[
            ("a", RuntimeState::WaitingInput),
            ("b", RuntimeState::WaitingInput),
            ("c", RuntimeState::Running),
            ("d", RuntimeState::Idle),
            ("e", RuntimeState::Exited),
            ("f", RuntimeState::Interrupted),
            ("g", RuntimeState::Error),
        ]);
        assert_eq!(badge_count(&s), Some(2));
    }

    #[test]
    fn badge_is_none_when_all_five_non_waiting_states_are_present() {
        // WaitingInput 以外の 5 値すべてを置く。部分列にしない —— 居ない値があると、
        // その値をフィルタに足す変異（誤って数えてしまう）が検出できなくなる。
        let s = states(&[
            ("a", RuntimeState::Running),
            ("b", RuntimeState::Idle),
            ("c", RuntimeState::Exited),
            ("d", RuntimeState::Interrupted),
            ("e", RuntimeState::Error),
        ]);
        assert_eq!(badge_count(&s), None);
    }

    #[test]
    fn waiting_input_title_follows_the_design_doc() {
        let label = SessionLabel {
            title: "fix-login".into(),
            project_name: "kamux".into(),
            location: "session/fix-login".into(),
        };
        let (title, body) = format_notification(NotifyKind::WaitingInput, &label);
        assert_eq!(title, "入力待ち: fix-login");
        assert_eq!(body, "kamux · session/fix-login");
    }

    #[test]
    fn stopped_title_follows_the_design_doc() {
        let label = SessionLabel {
            title: "fix-login".into(),
            project_name: "kamux".into(),
            location: "リポジトリ直上".into(),
        };
        let (title, body) = format_notification(NotifyKind::Stopped, &label);
        assert_eq!(title, "応答完了: fix-login");
        assert_eq!(body, "kamux · リポジトリ直上");
    }

    #[test]
    fn fallback_label_uses_a_short_session_id() {
        let label = SessionLabel::fallback("3f2a9c1e-0000-4000-8000-000000000000");
        assert_eq!(label.title, "セッション 3f2a9c1e");
        assert_eq!(label.project_name, "kamux");
        assert_eq!(label.location, "リポジトリ直上");
    }

    #[test]
    fn fallback_label_takes_eight_chars_not_eight_bytes() {
        // §81.2 変異検証: chars().take(8) をバイト単位のスライスに変異させると、
        // 全 ASCII の session_id では検出できない（"3f2a9c1e" は 8 バイト = 8 文字で一致してしまう）。
        // マルチバイト文字を含む session_id で「文字数」であることを固定する。
        let label = SessionLabel::fallback("日本語テストID-0000-4000-8000-000000000000");
        assert_eq!(label.title, "セッション 日本語テストID");
    }
}
