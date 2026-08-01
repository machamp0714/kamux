use serde::{Deserialize, Serialize};

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
}
