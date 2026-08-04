//! ブランチ名の提案。
//!
//! **提案時のみ**衝突を回避してサフィックスを付ける（設計判断 3）。
//! 実行時の `create_worktree` は一切リトライせず、git の stderr をそのまま返す。

use std::path::{Path, PathBuf};

use crate::error::AppResult;
use crate::worktree::slug::{title_slug, SLUG_MAX_CHARS};

/// ブランチ名のプレフィックス（設計書 §10 / 契約 §13）。
pub const BRANCH_PREFIX: &str = "session/";

/// 衝突回避で試すサフィックスの上限。
const MAX_SUFFIX: u32 = 99;

/// worktree の配置先（契約 §13）。ブランチ名が `session/x` でも
/// ディレクトリは `.worktrees/x` でフラットに置く。
pub fn worktree_path_for(repo_path: &Path, slug: &str) -> PathBuf {
    repo_path.join(".worktrees").join(slug)
}

/// ローカルブランチの存在確認。git の終了コードで判定する。
pub fn branch_exists(repo_path: &Path, branch: &str) -> bool {
    crate::worktree::run_git(
        repo_path,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// ベース slug に `-N` を付ける。合計が 40 文字を超えないようベース側を先に削る。
fn with_suffix(base: &str, n: u32) -> String {
    let suffix = format!("-{n}");
    let max_base = SLUG_MAX_CHARS.saturating_sub(suffix.chars().count());
    let trimmed: String = base.chars().take(max_base).collect();
    format!("{}{}", trimmed.trim_end_matches('-'), suffix)
}

fn is_taken(repo_path: &Path, slug: &str) -> bool {
    let branch = format!("{BRANCH_PREFIX}{slug}");
    branch_exists(repo_path, &branch) || worktree_path_for(repo_path, slug).exists()
}

/// 空いているブランチ名を提案する。ユーザーは UI 上でこれを編集できる。
/// 戻り値は `session/{slug}` という**ブランチ名そのもの**（契約 §60.2 で `BranchSuggestion` は却下）。
/// ディレクトリ名が要る呼び出し側は `branch_slug()` で導出する。
pub fn suggest_branch_name(repo_path: &Path, title: &str, session_id: &str) -> AppResult<String> {
    let base = title_slug(title, session_id);

    let slug = if is_taken(repo_path, &base) {
        let mut found = with_suffix(&base, MAX_SUFFIX);
        for n in 2..=MAX_SUFFIX {
            let candidate = with_suffix(&base, n);
            if !is_taken(repo_path, &candidate) {
                found = candidate;
                break;
            }
        }
        found
    } else {
        base
    };

    Ok(format!("{BRANCH_PREFIX}{slug}"))
}

/// `sessions.branch` から worktree ディレクトリ名を導く（契約 §60.2.2）。
///
/// `branch` は**ユーザーが手で編集できる入力値**であり §13 適合は保証されない（§51.3.2）。
/// したがって接頭辞を剥がすだけでは足りず、slug 規則を必ず通す。
/// 結果はつねに単一のフラットなパス構成要素になる（`/` を含まない、空でない、`.` / `..` にならない）。
pub fn branch_slug(branch: &str, session_id: &str) -> String {
    let stripped = branch.strip_prefix(BRANCH_PREFIX).unwrap_or(branch);
    title_slug(stripped, session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::slug::title_slug;
    use crate::worktree::test_support::TestRepo;

    const ID: &str = "3f2a1b9c-4d5e-6f70-8192-a3b4c5d6e7f8";

    #[test]
    fn suggests_prefixed_branch_from_title() {
        let repo = TestRepo::new();
        let branch = suggest_branch_name(repo.path(), "Fix login bug", ID).expect("suggest");
        assert_eq!(branch, "session/fix-login-bug");
    }

    #[test]
    fn appends_numeric_suffix_when_branch_exists() {
        let repo = TestRepo::new();
        repo.git(&["branch", "session/fix-login-bug"]);
        let branch = suggest_branch_name(repo.path(), "Fix login bug", ID).expect("suggest");
        assert_eq!(branch, "session/fix-login-bug-2");
    }

    #[test]
    fn skips_multiple_taken_suffixes() {
        let repo = TestRepo::new();
        repo.git(&["branch", "session/fix-login-bug"]);
        repo.git(&["branch", "session/fix-login-bug-2"]);
        let branch = suggest_branch_name(repo.path(), "Fix login bug", ID).expect("suggest");
        assert_eq!(branch, "session/fix-login-bug-3");
    }

    #[test]
    fn treats_existing_directory_as_collision() {
        let repo = TestRepo::new();
        // ブランチは無いが .worktrees/fix-login-bug が既にある場合も衝突扱い
        std::fs::create_dir_all(repo.path().join(".worktrees").join("fix-login-bug"))
            .expect("mkdir");
        let branch = suggest_branch_name(repo.path(), "Fix login bug", ID).expect("suggest");
        assert_eq!(branch, "session/fix-login-bug-2");
    }

    #[test]
    fn suffixed_slug_stays_within_40_chars() {
        let repo = TestRepo::new();
        let long_title = "a".repeat(60);
        let base = title_slug(&long_title, ID); // 40 文字ちょうど
        repo.git(&["branch", &format!("{BRANCH_PREFIX}{base}")]);
        let branch = suggest_branch_name(repo.path(), &long_title, ID).expect("suggest");
        let slug = branch_slug(&branch, ID);
        assert!(
            slug.chars().count() <= 40,
            "slug must stay within 40 chars, got {} chars: {}",
            slug.chars().count(),
            slug
        );
        assert!(slug.ends_with("-2"), "got {slug}");
    }

    // --- 契約 §60.2.2: branch_slug の導出と冪等性 ---

    #[test]
    fn branch_slug_strips_the_session_prefix() {
        assert_eq!(branch_slug("session/fix-login-bug", ID), "fix-login-bug");
    }

    #[test]
    fn branch_slug_flattens_a_user_edited_branch_with_slashes() {
        // ユーザーが手で `feature/foo` と打った場合。.worktrees/feature/foo という
        // **入れ子を作らせない**（契約 §60.2.2 / §51.3.2）
        let slug = branch_slug("feature/foo", ID);
        assert_eq!(slug, "feature-foo");
        assert!(!slug.contains('/'), "must be a single flat path component");
    }

    #[test]
    fn branch_slug_never_escapes_the_repository() {
        // `../x` がリポジトリ外への脱出を作らないこと
        for input in ["../x", "..", ".", "session/../..", "/etc/passwd"] {
            let slug = branch_slug(input, ID);
            assert!(
                !slug.contains('/'),
                "{input} produced a slug with a slash: {slug}"
            );
            assert!(!slug.is_empty(), "{input} produced an empty slug");
            assert_ne!(slug, ".", "{input} produced a bare dot");
            assert_ne!(slug, "..", "{input} produced a parent reference");
        }
    }

    #[test]
    fn branch_slug_falls_back_when_the_branch_has_no_alphanumerics() {
        assert_eq!(branch_slug("session/---", ID), "session-3f2a1b9c");
    }

    #[test]
    fn branch_slug_is_idempotent_over_suggest_branch_name() {
        // 契約 §60.2.2 が固定する唯一の性質:
        //   branch_slug(suggest_branch_name(..)?, id) == suggest_branch_name が内部で使った slug
        // 衝突回避サフィックスが付いた場合でも成り立つこと（40 文字境界を跨ぐため）
        let repo = TestRepo::new();
        for title in [
            "Fix login bug",
            "ログイン不具合の修正",
            "!!!",
            &"a".repeat(60),
        ] {
            let branch = suggest_branch_name(repo.path(), title, ID).expect("suggest");
            let slug = branch_slug(&branch, ID);
            assert_eq!(
                branch,
                format!("{BRANCH_PREFIX}{slug}"),
                "branch_slug is not idempotent for title {title:?}"
            );
            // 衝突させて -2 が付いた状態でも成り立つこと
            repo.git(&["branch", &branch]);
            let next = suggest_branch_name(repo.path(), title, ID).expect("suggest");
            let next_slug = branch_slug(&next, ID);
            assert_eq!(
                next,
                format!("{BRANCH_PREFIX}{next_slug}"),
                "not idempotent after collision"
            );
        }
    }

    #[test]
    fn worktree_path_is_flat_under_dot_worktrees() {
        // ブランチは session/xxx だがディレクトリは .worktrees/xxx（入れ子にしない）
        let p = worktree_path_for(std::path::Path::new("/repo"), "fix-login-bug");
        assert_eq!(
            p,
            std::path::PathBuf::from("/repo/.worktrees/fix-login-bug")
        );
    }

    #[test]
    fn branch_exists_reports_correctly() {
        let repo = TestRepo::new();
        repo.git(&["branch", "session/present"]);
        assert!(branch_exists(repo.path(), "session/present"));
        assert!(!branch_exists(repo.path(), "session/absent"));
    }
}
