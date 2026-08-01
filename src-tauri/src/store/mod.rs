use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{AppError, AppResult};

/// 契約 §0: DB は ~/Library/Application Support/kamux/app.db。
/// Tauri の app_data_dir() はバンドル identifier を含むパスを返すため使わない。
/// テストと将来の CI が本番 DB を汚さないよう、KAMUX_DB_PATH で上書きできる。
pub fn db_path() -> AppResult<PathBuf> {
    if let Some(overridden) = std::env::var_os("KAMUX_DB_PATH") {
        return Ok(PathBuf::from(overridden));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| AppError::Io("HOME environment variable is not set".to_owned()))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("kamux")
        .join("app.db"))
}

/// 契約 §3: 時刻は Unix epoch ミリ秒の INTEGER。
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 環境変数はプロセス全体で共有されるため、KAMUX_DB_PATH を触るテストは
    // このひとつだけに閉じ込める（他のテストは Store::open に明示パスを渡す）。
    #[test]
    fn db_path_honors_override_then_falls_back_to_application_support() {
        std::env::set_var("KAMUX_DB_PATH", "/tmp/kamux-override.db");
        assert_eq!(
            db_path().expect("path"),
            PathBuf::from("/tmp/kamux-override.db")
        );

        std::env::remove_var("KAMUX_DB_PATH");
        let home = std::env::var("HOME").expect("HOME");
        assert_eq!(
            db_path().expect("path"),
            PathBuf::from(home).join("Library/Application Support/kamux/app.db")
        );
    }

    #[test]
    fn now_ms_returns_a_plausible_epoch_millisecond() {
        let now = now_ms();
        // 2020-01-01T00:00:00Z より後で、ミリ秒スケールであること
        assert!(now > 1_577_836_800_000, "epoch ミリ秒になっていない: {now}");
        assert!(now < 4_102_444_800_000, "秒とミリ秒を取り違えている: {now}");
    }

    #[test]
    fn now_ms_is_monotonic_enough_for_updated_at() {
        let a = now_ms();
        std::thread::sleep(std::time::Duration::from_millis(3));
        assert!(now_ms() > a);
    }
}
