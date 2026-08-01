use serde::Serialize;

/// 契約 §6: 全 Tauri コマンドの Err 型。
/// Git / PtySpawn の message には加工していない stderr をそのまま入れる（設計書 §12）。
#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum AppError {
    #[error("db error: {0}")]
    Db(String),
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("pty spawn failed: {0}")]
    PtySpawn(String),
    #[error("git error: {0}")]
    Git(String),
    #[error("cli binary not found: {0}")]
    CliNotFound(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("io error: {0}")]
    Io(String),
}

pub type AppResult<T> = Result<T, AppError>;

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Db(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_to_code_and_message() {
        let json = serde_json::to_string(&AppError::NotFound("abc".into())).expect("serialize");
        assert_eq!(json, r#"{"code":"not_found","message":"abc"}"#);

        let json = serde_json::to_string(&AppError::Db("UNIQUE constraint failed".into()))
            .expect("serialize");
        assert_eq!(
            json,
            r#"{"code":"db","message":"UNIQUE constraint failed"}"#
        );

        let json =
            serde_json::to_string(&AppError::CliNotFound("claude".into())).expect("serialize");
        assert_eq!(json, r#"{"code":"cli_not_found","message":"claude"}"#);
    }

    #[test]
    fn all_variants_use_contract_codes() {
        let codes: Vec<String> = vec![
            AppError::Db(String::new()),
            AppError::NotFound(String::new()),
            AppError::PtySpawn(String::new()),
            AppError::Git(String::new()),
            AppError::CliNotFound(String::new()),
            AppError::InvalidState(String::new()),
            AppError::Io(String::new()),
        ]
        .into_iter()
        .map(|e| {
            let v = serde_json::to_value(&e).expect("serialize");
            v["code"].as_str().expect("code").to_owned()
        })
        .collect();

        assert_eq!(
            codes,
            vec![
                "db",
                "not_found",
                "pty_spawn",
                "git",
                "cli_not_found",
                "invalid_state",
                "io"
            ]
        );
    }

    #[test]
    fn rusqlite_error_converts_to_db_variant() {
        let conn = rusqlite::Connection::open_in_memory().expect("open");
        let err = conn.execute("SELECT * FROM nope", []).unwrap_err();
        let app: AppError = err.into();
        match app {
            AppError::Db(message) => assert!(message.contains("nope"), "stderr を握り潰していない"),
            other => panic!("expected Db, got {other:?}"),
        }
    }

    #[test]
    fn io_error_converts_to_io_variant() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let app: AppError = err.into();
        match app {
            AppError::Io(message) => assert_eq!(message, "no such file"),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn display_is_human_readable() {
        assert_eq!(
            AppError::Git("fatal: bad ref".into()).to_string(),
            "git error: fatal: bad ref"
        );
    }
}
