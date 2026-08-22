use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 契約 §2: serde 表現（snake_case 文字列）と DB に格納する文字列を
/// 1 箇所で定義するためのマクロ。両者がズレるとデータが読めなくなるため、
/// 一致は model.rs のテストで固定している。
macro_rules! db_enum {
    ($name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_db_str(self) -> &'static str {
                match self { $(Self::$variant => $s),+ }
            }

            pub fn from_db_str(s: &str) -> Option<Self> {
                match s {
                    $($s => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

db_enum!(KanbanStatus {
    Backlog => "backlog",
    InProgress => "in_progress",
    Review => "review",
    Done => "done",
});

db_enum!(SessionMode {
    Worktree => "worktree",
    InPlace => "in_place",
});

db_enum!(CliKind {
    Claude => "claude",
    Codex => "codex",
    Shell => "shell",
    Custom => "custom",
});

// 契約 §2 の 6 値。設計書 §5.3 の 5 値に `error` を加えたものが正典
// （設計書 §12 の「カードをエラー状態に」を満たす値が §5.3 に無いため）。
db_enum!(RuntimeState {
    Running => "running",
    WaitingInput => "waiting_input",
    Idle => "idle",
    Exited => "exited",
    Interrupted => "interrupted",
    Error => "error",
});

db_enum!(SurfaceKind {
    Agent => "agent",
    Editor => "editor",
});

/// 状態を変えた原因。UI のツールチップとデバッグ用（契約 §8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateReason {
    Spawned,
    HookNotification,
    HookStop,
    PtyExited,
    StartupNormalize,
    BelDetected,
    SilenceTimeout,
    UserStopped,
    OutputActivity, // PTY 出力活動による running 復帰（M2-1）
    UserInput,      // ユーザーのキー入力による waiting_input 解除（M2-1）
    HookPermission, // PermissionRequest hook 受信（契約 §12.4）
    ResumeFailed,   // resume 試行の失敗（M2-4）。StateInput::ResumeFailed から生成（契約 §41.3）
    SpawnFailed,    // error 状態への遷移（契約 §2。RuntimeSender::mark_error のみが発行。§40.2）
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionStatePayload {
    pub session_id: String,
    pub runtime_state: RuntimeState,
    pub reason: StateReason,
}

/// 通知クリックからセッションへフォーカスを戻すためのペイロード（契約 §8）。
#[derive(Serialize, Clone)]
pub struct FocusPayload {
    pub session_id: String,
    pub surface_kind: SurfaceKind,
}

/// 契約 §4
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub repo_path: String,
    pub default_cli: CliKind,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 契約 §4
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub kanban_status: KanbanStatus,
    pub sort_order: f64,
    pub mode: SessionMode,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub cli_kind: CliKind,
    pub cli_command: Option<String>,
    pub claude_session_id: Option<String>,
    pub last_runtime_state: RuntimeState,
    /// `error` 状態のときの生 stderr。加工しない。error 以外へ遷移したら None に戻す（契約 §4 / §17）
    pub last_runtime_error: Option<String>,
    /// 最初に PTY spawn に成功した時刻（epoch ms）。None = 一度も起動していない。
    /// 一度書いたら上書きしない（契約 §34）。書き手は M2-1 の SessionManager だけ
    pub first_started_at: Option<i64>,
    /// 汎用 CLI ヒューリスティック検知のオン/オフ（契約 §20 / M3-3）。
    /// 既定値は `cli_kind` ごとに `new_backlog` が決める（設計 §4.6）
    pub heuristics_enabled: bool,
    /// 沈黙タイムアウト（秒）。許容範囲は
    /// `MIN_SILENCE_TIMEOUT_SECS..=MAX_SILENCE_TIMEOUT_SECS`（契約 §20 / M3-3）
    pub silence_timeout_secs: u32,
    pub archived_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Session {
    /// 新規セッションの初期値。`now` を引数で受けるのは、`now_ms()` が store 側にあり
    /// model -> store の依存を作りたくないため。純関数なのでテストしやすい。
    // 引数 9 個は契約 §17 のシグネチャそのもの。分割すると DAO 境界（呼び出し側での
    // 組み立てという契約 §17 の構造）が変わるため、まとめずそのまま許可する。
    #[allow(clippy::too_many_arguments)]
    pub fn new_backlog(
        project_id: &str,
        title: &str,
        description: &str,
        mode: SessionMode,
        branch: Option<String>,
        cli_kind: CliKind,
        cli_command: Option<String>,
        sort_order: f64,
        now: i64,
    ) -> Session {
        Session {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_owned(),
            title: title.to_owned(),
            description: description.to_owned(),
            kanban_status: KanbanStatus::Backlog,
            sort_order,
            mode,
            branch,
            worktree_path: None,
            cli_kind,
            cli_command,
            claude_session_id: None,
            last_runtime_state: RuntimeState::Idle,
            last_runtime_error: None,
            first_started_at: None,
            // 既定値を決めるのはこの構築点だけ（契約 §20 / 設計 §4.6）。
            // DAO 側で cli_kind から再計算すると構造体と DB が食い違う
            heuristics_enabled: crate::session::heuristics::registry::default_heuristics_enabled(
                cli_kind,
            ),
            silence_timeout_secs: crate::session::heuristics::DEFAULT_SILENCE_TIMEOUT_SECS,
            archived_at: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// フィールドが「不在」なのか「null が明示された」のかを区別するためのデシリアライザ。
/// 不在   -> #[serde(default)] により None
/// null   -> Some(None)
/// 値あり -> Some(Some(v))
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

/// 契約 §7: 部分更新。None のフィールドは変更しない。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SessionPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub kanban_status: Option<KanbanStatus>,
    #[serde(default)]
    pub sort_order: Option<f64>,
    #[serde(default, deserialize_with = "double_option")]
    pub archived_at: Option<Option<i64>>,
    /// 契約 §20（M3-3）。NOT NULL 列なので「null 明示」に意味は無く、
    /// archived_at のような `Option<Option<_>>` にはしない
    #[serde(default)]
    pub heuristics_enabled: Option<bool>,
    #[serde(default)]
    pub silence_timeout_secs: Option<u32>,
    /// システム由来値。クリア（Some(None)）のみ許可し、値の設定は拒否する（第1部 §4.9）。
    #[serde(default, deserialize_with = "double_option")]
    pub claude_session_id: Option<Option<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_roundtrip<T>(value: T, expected: &str)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug + PartialEq + Copy,
    {
        let json = serde_json::to_string(&value).expect("serialize");
        assert_eq!(
            json,
            format!("\"{expected}\""),
            "serde 表現が契約 §2 と違う"
        );
        let back: T = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, value);
    }

    #[test]
    fn kanban_status_matches_contract_strings() {
        assert_roundtrip(KanbanStatus::Backlog, "backlog");
        assert_roundtrip(KanbanStatus::InProgress, "in_progress");
        assert_roundtrip(KanbanStatus::Review, "review");
        assert_roundtrip(KanbanStatus::Done, "done");
    }

    #[test]
    fn session_mode_and_cli_kind_match_contract_strings() {
        assert_roundtrip(SessionMode::Worktree, "worktree");
        assert_roundtrip(SessionMode::InPlace, "in_place");
        assert_roundtrip(CliKind::Claude, "claude");
        assert_roundtrip(CliKind::Codex, "codex");
        assert_roundtrip(CliKind::Shell, "shell");
        assert_roundtrip(CliKind::Custom, "custom");
    }

    #[test]
    fn runtime_state_and_surface_kind_match_contract_strings() {
        assert_roundtrip(RuntimeState::Running, "running");
        assert_roundtrip(RuntimeState::WaitingInput, "waiting_input");
        assert_roundtrip(RuntimeState::Idle, "idle");
        assert_roundtrip(RuntimeState::Exited, "exited");
        assert_roundtrip(RuntimeState::Interrupted, "interrupted");
        assert_roundtrip(RuntimeState::Error, "error"); // 契約 §2 の 6 値目
        assert_roundtrip(SurfaceKind::Agent, "agent");
        assert_roundtrip(SurfaceKind::Editor, "editor");
    }

    /// `StateReason` は `db_enum!` を使わない手書き enum なので、上の
    /// `assert_roundtrip` が保証する `db_enum!` 型の一致とは別に固定する必要がある
    /// （`Deserialize` を持たないため `assert_roundtrip` は使えない）。
    /// 契約 §33.2 の TS union（13 個の snake_case 文字列）と 1 文字も違わないこと。
    #[test]
    fn state_reason_matches_contract_strings() {
        let cases: [(StateReason, &str); 13] = [
            (StateReason::Spawned, "spawned"),
            (StateReason::HookNotification, "hook_notification"),
            (StateReason::HookStop, "hook_stop"),
            (StateReason::PtyExited, "pty_exited"),
            (StateReason::StartupNormalize, "startup_normalize"),
            (StateReason::BelDetected, "bel_detected"),
            (StateReason::SilenceTimeout, "silence_timeout"),
            (StateReason::UserStopped, "user_stopped"),
            (StateReason::OutputActivity, "output_activity"),
            (StateReason::UserInput, "user_input"),
            (StateReason::HookPermission, "hook_permission"),
            (StateReason::ResumeFailed, "resume_failed"),
            (StateReason::SpawnFailed, "spawn_failed"),
        ];
        for (value, expected) in cases {
            let json = serde_json::to_string(&value).expect("serialize");
            assert_eq!(
                json,
                format!("\"{expected}\""),
                "StateReason の serde 表現が契約 §33.2 と違う: {value:?}"
            );
        }
    }

    /// serde 表現（`#[serde(rename_all = "snake_case")]` による自動導出）と
    /// DB 側の文字列（`db_enum!` 呼び出しごとに手書きする `$s` リテラル）は
    /// マクロが同一性を強制しない独立の source。この macro で、各型ごとに
    /// 「全 variant で serde 表現 == `as_db_str()`」と「`from_db_str` が
    /// `as_db_str` の逆変換になっている」ことを検証する。ジェネリックな
    /// ヘルパにすると `db_enum!` にトレイトを生やす必要が出るため、
    /// 型ごとにループを展開する素朴な形にしている（`db_enum!` 本体は
    /// 触らない）。
    macro_rules! assert_db_str_matches_serde {
        ($ty:ty, [$($v:expr),+ $(,)?]) => {
            for v in [$($v),+] {
                let json = serde_json::to_string(&v).expect("serialize");
                assert_eq!(
                    json,
                    format!("\"{}\"", v.as_db_str()),
                    "serde 表現と as_db_str がズレている: {v:?}"
                );
                assert_eq!(<$ty>::from_db_str(v.as_db_str()), Some(v));
            }
        };
    }

    #[test]
    fn db_str_equals_serde_representation() {
        // DB に入れる文字列と serde の文字列がズレたらデータが読めなくなる。
        // 5 型・全 18 variant を網羅する。
        assert_db_str_matches_serde!(
            KanbanStatus,
            [
                KanbanStatus::Backlog,
                KanbanStatus::InProgress,
                KanbanStatus::Review,
                KanbanStatus::Done,
            ]
        );
        assert_db_str_matches_serde!(SessionMode, [SessionMode::Worktree, SessionMode::InPlace]);
        assert_db_str_matches_serde!(
            CliKind,
            [
                CliKind::Claude,
                CliKind::Codex,
                CliKind::Shell,
                CliKind::Custom,
            ]
        );
        assert_db_str_matches_serde!(
            RuntimeState,
            [
                RuntimeState::Running,
                RuntimeState::WaitingInput,
                RuntimeState::Idle,
                RuntimeState::Exited,
                RuntimeState::Interrupted,
                RuntimeState::Error,
            ]
        );
        assert_db_str_matches_serde!(SurfaceKind, [SurfaceKind::Agent, SurfaceKind::Editor]);
    }

    #[test]
    fn from_db_str_rejects_unknown_values() {
        assert_eq!(KanbanStatus::from_db_str("archived"), None);
        assert_eq!(CliKind::from_db_str(""), None);
        assert_eq!(RuntimeState::from_db_str("Running"), None); // 大文字は別物
    }

    fn sample_session() -> Session {
        Session {
            id: "sid".into(),
            project_id: "pid".into(),
            title: "fix login".into(),
            description: String::new(),
            kanban_status: KanbanStatus::InProgress,
            sort_order: 2.5,
            mode: SessionMode::InPlace,
            branch: None,
            worktree_path: None,
            cli_kind: CliKind::Claude,
            cli_command: None,
            claude_session_id: None,
            last_runtime_state: RuntimeState::Idle,
            last_runtime_error: None,
            first_started_at: None,
            heuristics_enabled: true,
            silence_timeout_secs: 30,
            archived_at: None,
            created_at: 1,
            updated_at: 2,
        }
    }

    #[test]
    fn session_serializes_with_snake_case_field_names() {
        let v = serde_json::to_value(sample_session()).expect("serialize");
        assert_eq!(v["kanban_status"], "in_progress");
        assert_eq!(v["last_runtime_state"], "idle");
        assert_eq!(v["sort_order"], 2.5);
        assert_eq!(v["worktree_path"], serde_json::Value::Null);
        assert_eq!(v["claude_session_id"], serde_json::Value::Null);
        assert_eq!(v["last_runtime_error"], serde_json::Value::Null);
        assert_eq!(v["archived_at"], serde_json::Value::Null);
        // 契約 §20（M3-3）。フロントの `Session` はこの 2 キーを読む
        assert_eq!(v["heuristics_enabled"], true);
        assert_eq!(v["silence_timeout_secs"], 30);
        // 契約 §22: 禁止名が漏れていないこと
        assert!(v.get("status").is_none());
        assert!(v.get("state").is_none());
    }

    #[test]
    fn project_serializes_with_snake_case_field_names() {
        let p = Project {
            id: "pid".into(),
            name: "kamux".into(),
            repo_path: "/Users/x/repo/kamux".into(),
            default_cli: CliKind::Claude,
            created_at: 10,
            updated_at: 20,
        };
        let v = serde_json::to_value(p).expect("serialize");
        assert_eq!(v["repo_path"], "/Users/x/repo/kamux");
        assert_eq!(v["default_cli"], "claude");
    }

    #[test]
    fn session_patch_absent_fields_are_none() {
        let patch: SessionPatch = serde_json::from_str("{}").expect("deserialize");
        assert!(patch.title.is_none());
        assert!(patch.kanban_status.is_none());
        assert!(patch.sort_order.is_none());
        assert!(patch.archived_at.is_none(), "不在は「変更しない」");
        assert!(patch.heuristics_enabled.is_none());
        assert!(patch.silence_timeout_secs.is_none());
    }

    #[test]
    fn session_patch_distinguishes_absent_null_and_value_for_archived_at() {
        let absent: SessionPatch = serde_json::from_str(r#"{"title":"t"}"#).expect("deserialize");
        assert_eq!(absent.archived_at, None, "不在 = 変更しない");

        let cleared: SessionPatch =
            serde_json::from_str(r#"{"archived_at":null}"#).expect("deserialize");
        assert_eq!(cleared.archived_at, Some(None), "null = アーカイブ解除");

        let set: SessionPatch =
            serde_json::from_str(r#"{"archived_at":1700000000000}"#).expect("deserialize");
        assert_eq!(
            set.archived_at,
            Some(Some(1700000000000)),
            "数値 = アーカイブ"
        );
    }

    /// M2-4: `claude_session_id` は `archived_at` と同じ 3 分岐（不在 / null / 値あり）を
    /// 区別しなければならない（`deserialize_with = "double_option"` が要る理由）。
    #[test]
    fn session_patch_distinguishes_absent_null_and_value_for_claude_session_id() {
        let absent: SessionPatch = serde_json::from_str(r#"{"title":"t"}"#).expect("deserialize");
        assert_eq!(absent.claude_session_id, None, "不在 = 変更しない");

        let cleared: SessionPatch =
            serde_json::from_str(r#"{"claude_session_id":null}"#).expect("deserialize");
        assert_eq!(cleared.claude_session_id, Some(None), "null = クリア要求");

        let set: SessionPatch =
            serde_json::from_str(r#"{"claude_session_id":"abc"}"#).expect("deserialize");
        assert_eq!(
            set.claude_session_id,
            Some(Some("abc".to_string())),
            "値あり = 設定要求（validate_session_patch が拒否する）"
        );
    }

    #[test]
    fn session_patch_reads_snake_case_keys() {
        let patch: SessionPatch = serde_json::from_str(
            r#"{"kanban_status":"review","sort_order":1.5,
                "heuristics_enabled":false,"silence_timeout_secs":120}"#,
        )
        .expect("deserialize");
        assert_eq!(patch.kanban_status, Some(KanbanStatus::Review));
        assert_eq!(patch.sort_order, Some(1.5));
        assert_eq!(patch.heuristics_enabled, Some(false));
        assert_eq!(patch.silence_timeout_secs, Some(120));
    }

    #[test]
    fn new_backlog_fills_the_creation_time_defaults() {
        let s = Session::new_backlog(
            "pid",
            "fix login",
            "壊れたログインを直す",
            SessionMode::InPlace,
            None,
            CliKind::Claude,
            None,
            3.0,
            1700000000000,
        );

        assert_eq!(s.project_id, "pid");
        assert_eq!(s.title, "fix login");
        assert_eq!(s.description, "壊れたログインを直す");
        assert_eq!(s.kanban_status, KanbanStatus::Backlog);
        assert_eq!(s.last_runtime_state, RuntimeState::Idle);
        assert_eq!(s.sort_order, 3.0);
        assert_eq!(s.mode, SessionMode::InPlace);
        assert_eq!(s.branch, None);
        assert_eq!(s.worktree_path, None, "worktree 作成は M1-4");
        assert_eq!(s.claude_session_id, None, "session_id 捕捉は M2-2");
        assert_eq!(s.archived_at, None);
        assert_eq!(s.id.len(), 36, "UUID v4 のハイフン付き文字列");
        assert_eq!(s.created_at, 1700000000000);
        assert_eq!(s.updated_at, 1700000000000, "作成時は両方同じ");
    }

    /// `cli_kind` 別の既定値を決める場所は `new_backlog` **だけ**である（契約 §20 / 設計 §4.6）。
    /// DAO 側（`insert_session`）で既定値を再計算すると、`Session` が
    /// `heuristics_enabled: false` を持っていても DB には true が書かれ、
    /// 構造体と DB が食い違う。DAO 側は素通しであることを
    /// `insert_session_persists_the_sessions_own_heuristics_settings` が固定している。
    #[test]
    fn new_backlog_takes_the_heuristics_default_from_the_cli_kind() {
        fn built_with(cli_kind: CliKind) -> Session {
            Session::new_backlog(
                "p",
                "t",
                "",
                SessionMode::InPlace,
                None,
                cli_kind,
                None,
                1.0,
                1,
            )
        }

        assert!(
            !built_with(CliKind::Shell).heuristics_enabled,
            "shell は既定オフ（対話シェルは補完失敗のたびに BEL を鳴らす）"
        );
        assert!(
            built_with(CliKind::Claude).heuristics_enabled,
            "claude は既定オン"
        );
        assert!(
            built_with(CliKind::Codex).heuristics_enabled,
            "codex は既定オン"
        );
        assert!(
            built_with(CliKind::Custom).heuristics_enabled,
            "custom は既定オン"
        );
    }

    #[test]
    fn new_backlog_uses_the_default_silence_timeout() {
        let s = Session::new_backlog(
            "p",
            "t",
            "",
            SessionMode::InPlace,
            None,
            CliKind::Claude,
            None,
            1.0,
            1,
        );
        assert_eq!(
            s.silence_timeout_secs,
            crate::session::heuristics::DEFAULT_SILENCE_TIMEOUT_SECS
        );
        assert_eq!(
            s.silence_timeout_secs, 30,
            "契約 §20 の DDL の DEFAULT 30 と同じ値であること"
        );
    }

    #[test]
    fn new_backlog_generates_a_unique_id_each_call() {
        let a = Session::new_backlog(
            "p",
            "t",
            "",
            SessionMode::InPlace,
            None,
            CliKind::Shell,
            None,
            1.0,
            1,
        );
        let b = Session::new_backlog(
            "p",
            "t",
            "",
            SessionMode::InPlace,
            None,
            CliKind::Shell,
            None,
            1.0,
            1,
        );
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn new_backlog_carries_worktree_and_custom_cli_fields() {
        let s = Session::new_backlog(
            "pid",
            "fix login",
            "",
            SessionMode::Worktree,
            Some("session/fix-login".to_owned()),
            CliKind::Custom,
            Some("bun run agent".to_owned()),
            1.0,
            1,
        );

        assert_eq!(s.mode, SessionMode::Worktree);
        assert_eq!(s.branch.as_deref(), Some("session/fix-login"));
        assert_eq!(s.cli_command.as_deref(), Some("bun run agent"));
        assert_eq!(
            s.worktree_path, None,
            "worktree_path は set_worktree（M1-4）で入る"
        );
    }
}
