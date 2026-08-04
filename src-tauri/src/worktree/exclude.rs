//! `.git/info/exclude` への `.worktrees/` 追記（設計書 §10 / 契約 §13）。
//!
//! ユーザーの `.gitignore` は変更しない。`.git/info/exclude` はリポジトリ
//! ローカルかつコミット対象外なので、他の開発者に影響しない。

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::AppResult;
use crate::worktree::run_git;

/// exclude に書く 1 行（契約 §13）。
pub const EXCLUDE_ENTRY: &str = ".worktrees/";

/// 共通 `.git` ディレクトリの絶対パスを返す。
///
/// `git rev-parse --git-common-dir` はリポジトリ直上では相対パス `.git` を、
/// worktree の中からは絶対パスを返す。相対の場合は `repo_path` に接いで
/// 必ず絶対パスにする。`--git-dir` ではなく `--git-common-dir` を使うのは、
/// worktree 内から呼ばれても本体の `.git` を指すため。
pub fn git_common_dir(repo_path: &Path) -> AppResult<PathBuf> {
    let raw = run_git(repo_path, &["rev-parse", "--git-common-dir"])?;
    let trimmed = raw.trim();
    let candidate = PathBuf::from(trimmed);
    Ok(if candidate.is_absolute() {
        candidate
    } else {
        repo_path.join(candidate)
    })
}

/// `.worktrees/` を `.git/info/exclude` に 1 度だけ追記する。
/// 既に同じ行があれば何もしない。
pub fn ensure_worktrees_excluded(repo_path: &Path) -> AppResult<()> {
    let info_dir = git_common_dir(repo_path)?.join("info");
    fs::create_dir_all(&info_dir)?;

    let exclude_path = info_dir.join("exclude");
    // ファイルが存在しない(NotFound)は正常系として空文字列扱いにする。
    // それ以外の読み取りエラー(権限・非 UTF-8 等)はここで止める。
    // 握り潰して空文字列にすると、直後の書き込みでユーザーの既存
    // .git/info/exclude(git 管理外で復元手段が無い)を丸ごと上書きしてしまう。
    let current = match fs::read_to_string(&exclude_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };

    // 部分文字列一致ではなく、行単位・前後空白除去の完全一致で判定する。
    // "#.worktrees/"（コメント）や "foo/.worktrees/bar"（無関係な行）を
    // 「既にある」と誤判定しないため。
    if current.lines().any(|line| line.trim() == EXCLUDE_ENTRY) {
        return Ok(());
    }

    let mut next = current;
    // 既存ファイルが末尾改行を持たない場合に連結してしまわないよう、
    // 追記前に改行を補う。
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(EXCLUDE_ENTRY);
    next.push('\n');

    fs::write(&exclude_path, next)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::test_support::TestRepo;

    fn exclude_contents(repo: &TestRepo) -> String {
        std::fs::read_to_string(repo.path().join(".git").join("info").join("exclude"))
            .expect("read exclude")
    }

    #[test]
    fn git_common_dir_resolves_relative_output_to_absolute() {
        // リポジトリ直上では git は ".git"（相対）を返す。絶対パスに解決すること。
        let repo = TestRepo::new();
        let dir = git_common_dir(repo.path()).expect("common dir");
        assert!(dir.is_absolute(), "expected absolute path, got {dir:?}");
        assert!(dir.ends_with(".git"), "got {dir:?}");
        assert!(dir.exists());
    }

    #[test]
    fn git_common_dir_points_at_the_main_git_dir_from_inside_a_worktree() {
        // worktree の中から呼んでも、worktree 専用ディレクトリ
        // (.git/worktrees/<name>) ではなく本体の .git を指すこと
        // (--git-common-dir を使う理由そのもの)。
        let repo = TestRepo::new();
        repo.git(&["worktree", "add", "-b", "probe", "wt-probe"]);

        let dir = git_common_dir(&repo.path().join("wt-probe")).expect("common dir");

        assert!(dir.is_absolute(), "expected absolute path, got {dir:?}");
        assert_eq!(
            std::fs::canonicalize(&dir).expect("canonicalize actual"),
            std::fs::canonicalize(repo.path().join(".git")).expect("canonicalize expected"),
            "must resolve to the main repo's .git, got {dir:?}"
        );
    }

    #[test]
    fn ensure_worktrees_excluded_writes_to_the_main_repo_from_inside_a_worktree() {
        let repo = TestRepo::new();
        repo.git(&["worktree", "add", "-b", "probe2", "wt-probe2"]);

        ensure_worktrees_excluded(&repo.path().join("wt-probe2")).expect("exclude from worktree");

        let body = exclude_contents(&repo);
        assert_eq!(
            body.lines().filter(|l| l.trim() == EXCLUDE_ENTRY).count(),
            1,
            "entry must land in the main repo's .git/info/exclude, got:\n{body}"
        );
    }

    #[test]
    fn treats_a_whitespace_padded_existing_entry_as_already_present() {
        let repo = TestRepo::new();
        let path = repo.path().join(".git").join("info").join("exclude");
        std::fs::write(&path, ".worktrees/  \n").expect("seed padded entry");

        ensure_worktrees_excluded(repo.path()).expect("no-op");

        let body = exclude_contents(&repo);
        assert_eq!(
            body.lines().count(),
            1,
            "must not append a duplicate, got:\n{body}"
        );
    }

    #[test]
    fn refuses_to_overwrite_when_existing_exclude_is_not_valid_utf8() {
        // 非 UTF-8 の既存ファイルは read_to_string がエラーになる。これを
        // unwrap_or_default() で握り潰すと、直後の write が既存の中身を
        // .worktrees/ だけの内容で上書きしてしまう(データ破壊)。エラーで
        // 止まり、既存ファイルが 1 バイトも変わっていないことを確認する。
        let repo = TestRepo::new();
        let path = repo.path().join(".git").join("info").join("exclude");
        let original: &[u8] = b"\xff\xfe invalid utf8\n";
        std::fs::write(&path, original).expect("seed invalid utf8");

        let result = ensure_worktrees_excluded(repo.path());

        assert!(result.is_err(), "expected an error, got {result:?}");
        let after = std::fs::read(&path).expect("read back raw bytes");
        assert_eq!(
            after, original,
            "existing exclude file must not be modified on read error"
        );
    }

    #[test]
    fn creates_exclude_file_when_it_does_not_exist_yet() {
        // `git init` はデフォルトで .git/info/exclude にサンプルコメント入りの
        // テンプレートを作る。「存在しない」経路を確実に踏むため明示的に消す。
        let repo = TestRepo::new();
        let path = repo.path().join(".git").join("info").join("exclude");
        std::fs::remove_file(&path).expect("remove default template exclude file");
        assert!(!path.exists(), "precondition: exclude must not exist yet");

        ensure_worktrees_excluded(repo.path()).expect("create new exclude file");

        let body = exclude_contents(&repo);
        assert_eq!(body, format!("{EXCLUDE_ENTRY}\n"));
    }

    #[test]
    fn appends_entry_once() {
        let repo = TestRepo::new();
        ensure_worktrees_excluded(repo.path()).expect("first");
        let body = exclude_contents(&repo);
        assert_eq!(
            body.lines().filter(|l| l.trim() == EXCLUDE_ENTRY).count(),
            1
        );
    }

    #[test]
    fn is_idempotent_across_repeated_calls() {
        let repo = TestRepo::new();
        ensure_worktrees_excluded(repo.path()).expect("first");
        ensure_worktrees_excluded(repo.path()).expect("second");
        ensure_worktrees_excluded(repo.path()).expect("third");
        let body = exclude_contents(&repo);
        assert_eq!(
            body.lines().filter(|l| l.trim() == EXCLUDE_ENTRY).count(),
            1,
            "entry must not be duplicated, got:\n{body}"
        );
    }

    #[test]
    fn preserves_existing_lines_and_adds_missing_newline() {
        let repo = TestRepo::new();
        let path = repo.path().join(".git").join("info").join("exclude");
        std::fs::write(&path, "*.log").expect("seed without trailing newline");

        ensure_worktrees_excluded(repo.path()).expect("append");

        let body = exclude_contents(&repo);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines[0], "*.log");
        assert!(lines.contains(&EXCLUDE_ENTRY), "got:\n{body}");
    }

    #[test]
    fn actually_excludes_the_directory_from_git_status() {
        let repo = TestRepo::new();
        ensure_worktrees_excluded(repo.path()).expect("exclude");
        std::fs::create_dir_all(repo.path().join(".worktrees").join("x")).expect("mkdir");
        std::fs::write(repo.path().join(".worktrees").join("x").join("f"), "y").expect("write");

        let status = repo.git(&["status", "--porcelain"]);
        assert!(
            !status.contains(".worktrees"),
            "worktrees dir must be ignored, git status said:\n{status}"
        );
    }

    #[test]
    fn does_not_misfire_on_commented_or_unrelated_substring_matches() {
        // 部分文字列一致で判定すると、コメント行や無関係な行の中に
        // ".worktrees/" という文字列が含まれるだけで「既にある」と誤判定する。
        // 行単位の完全一致で判定していることを確認する。
        let repo = TestRepo::new();
        let path = repo.path().join(".git").join("info").join("exclude");
        std::fs::write(&path, "#.worktrees/\nfoo/.worktrees/bar\n").expect("seed decoys");

        ensure_worktrees_excluded(repo.path()).expect("append despite decoys");

        let body = exclude_contents(&repo);
        assert_eq!(
            body.lines().filter(|l| l.trim() == EXCLUDE_ENTRY).count(),
            1,
            "real entry must be added exactly once despite decoy lines, got:\n{body}"
        );
        assert!(
            body.contains("#.worktrees/"),
            "decoy comment line must survive, got:\n{body}"
        );
        assert!(
            body.contains("foo/.worktrees/bar"),
            "decoy unrelated line must survive, got:\n{body}"
        );
    }
}
