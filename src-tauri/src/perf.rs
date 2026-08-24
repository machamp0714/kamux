//! 起動時間計装。契約 §0 の「起動 1 秒未満」を再現可能に測るための最小限のログ。
//! ポーリングは一切しない（イベント発生時にのみ 1 行書く）。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();

/// `run()` の一番最初に呼ぶ。2 回目以降の呼び出しは無視される
/// （`OnceLock::set` は既に値が入っていれば `Err` を返すだけで上書きしない）。
pub fn mark_process_start() {
    let _ = PROCESS_START.set(Instant::now());
}

/// `mark_process_start()` からの経過ミリ秒。未設定なら 0。
pub fn elapsed_ms() -> u128 {
    PROCESS_START
        .get()
        .map(|t| t.elapsed().as_millis())
        .unwrap_or(0)
}

/// `~/Library/Application Support/kamux/perf.log`（契約 §0 の DB と同じディレクトリ）
pub fn log_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join("Library/Application Support/kamux/perf.log")
}

/// scripts/measure-perf.sh が解析する 1 行の形式
pub fn format_line(event: &str, ms: u128) -> String {
    format!("[kamux-perf] {event}={ms}\n")
}

/// 計測点を記録する。失敗しても計測のためだけの機能なのでアプリは止めない。
///
/// 実際の書き込みは `record_to` に委譲する。`record_to` は固定パス
/// `log_path()` に依存しないので、テストから一時ディレクトリを渡して
/// ユーザーの実ファイルを汚さずに検証できる（この分離自体がテスト対象。
/// `record()`（引数なしの側）を呼ぶテストは書かない —— 実ファイルに書くため）。
pub fn record(event: &str) {
    record_to(&log_path(), event);
}

/// `record` の本体。`path` を注入できるのでテスト専用の一時ファイルへ書ける。
fn record_to(path: &Path, event: &str) {
    let line = format_line(event, elapsed_ms());
    eprint!("{line}");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_ms_grows_after_mark_process_start() {
        mark_process_start();
        let a = elapsed_ms();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = elapsed_ms();
        assert!(b >= a, "経過時間が巻き戻った: {a} -> {b}");
        assert!(b >= 20, "20ms 待ったのに経過が {b}ms");
    }

    #[test]
    fn log_path_is_under_application_support_kamux() {
        let p = log_path();
        let s = p.to_string_lossy();
        assert!(
            s.ends_with("Library/Application Support/kamux/perf.log"),
            "ログパスが契約 §0 の DB ディレクトリと揃っていない: {s}"
        );
    }

    #[test]
    fn format_line_matches_the_parsed_shape() {
        // scripts/measure-perf.sh が `grep 'xxx_ms=' | sed 's/.*=//'` で読む形式
        let line = format_line("frontend_ready_ms", 842);
        assert_eq!(line, "[kamux-perf] frontend_ready_ms=842\n");
    }

    #[test]
    fn record_to_appends_lines_instead_of_overwriting() {
        let dir = tempfile::tempdir().expect("tempdir");
        // 親ディレクトリが存在しない状態から書かせる。record_to が
        // create_dir_all を省略すると 1 回目の書き込みから静かに失敗する
        // （group 4 の自己設計変異が狙う箇所）。
        let path = dir.path().join("nested").join("perf.log");

        record_to(&path, "first_ms");
        record_to(&path, "second_ms");

        let contents = std::fs::read_to_string(&path).expect("perf.log を読めなかった");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "record_to は追記のはずだが行数が {}: {contents:?}",
            lines.len()
        );
        assert!(
            lines[0].contains("first_ms="),
            "1 行目に first_ms が無い: {contents:?}"
        );
        assert!(
            lines[1].contains("second_ms="),
            "2 行目に second_ms が無い: {contents:?}"
        );
    }
}
