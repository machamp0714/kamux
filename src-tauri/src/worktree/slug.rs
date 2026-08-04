//! ブランチ名 / worktree ディレクトリ名に使う slug の生成。
//!
//! 契約 §13（正典は §60.5 の 6 手順版）の規則:
//!   小文字化 → 英数字とハイフン以外を '-' に → 連続ハイフン圧縮
//!   → 前後ハイフン除去 → 40 文字切り詰め → 切り詰め後にもう一度前後ハイフン除去
//!   空になった場合は "session-{id の先頭 8 文字}"
//!
//! 「英数字」は **ASCII 英数字のみ**。日本語などの非 ASCII は '-' に落ちるため、
//! 日本語のみのタイトルは必ずフォールバックする（設計判断 2）。
//!
//! 切り詰め後の再トリムは契約 §13 の条文にまだ明記されていないが、
//! team-lead の暫定裁定により本実装で確定させる（テスト
//! `truncates_to_40_chars_then_retrims_trailing_hyphen` がこの挙動を担保する）。

/// slug の最大文字数（契約 §13）。
pub const SLUG_MAX_CHARS: usize = 40;

pub fn title_slug(title: &str, session_id: &str) -> String {
    let mut hyphenated = String::with_capacity(title.len());
    for ch in title.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            hyphenated.push(ch);
        } else {
            hyphenated.push('-');
        }
    }

    // 連続ハイフン圧縮
    let mut compressed = String::with_capacity(hyphenated.len());
    let mut prev_hyphen = false;
    for ch in hyphenated.chars() {
        if ch == '-' {
            if !prev_hyphen {
                compressed.push(ch);
            }
            prev_hyphen = true;
        } else {
            compressed.push(ch);
            prev_hyphen = false;
        }
    }

    // 前後ハイフン除去 → 切り詰め → 切り詰めで再出現した端のハイフンを除去
    let truncated: String = compressed
        .trim_matches('-')
        .chars()
        .take(SLUG_MAX_CHARS)
        .collect();
    let slug = truncated.trim_matches('-');

    if slug.is_empty() {
        let prefix: String = session_id.chars().take(8).collect();
        format!("session-{prefix}")
    } else {
        slug.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "3f2a1b9c-4d5e-6f70-8192-a3b4c5d6e7f8";

    #[test]
    fn lowercases_and_hyphenates() {
        assert_eq!(title_slug("Fix Login Bug", ID), "fix-login-bug");
    }

    #[test]
    fn compresses_runs_and_trims_edges() {
        assert_eq!(title_slug("  Fix  ---  Login!! ", ID), "fix-login");
    }

    #[test]
    fn non_ascii_only_title_falls_back_to_session_id_prefix() {
        // 日本語のみ: 全文字が '-' 化 → 圧縮 → トリム → 空 → フォールバック（判断 2）
        assert_eq!(title_slug("ログイン不具合の修正", ID), "session-3f2a1b9c");
    }

    #[test]
    fn keeps_ascii_part_of_mixed_title() {
        assert_eq!(title_slug("日本語 fix login", ID), "fix-login");
    }

    #[test]
    fn truncates_to_40_chars_then_retrims_trailing_hyphen() {
        // 39 文字 + 空白 + "zzz" → 正規化後 "a"*39 + "-zzz"
        // 40 文字切り詰めで "a"*39 + "-" になり、再トリムで "a"*39 になること
        let title = format!("{} zzz", "a".repeat(39));
        let out = title_slug(&title, ID);
        assert_eq!(out, "a".repeat(39));
        assert!(
            !out.ends_with('-'),
            "trailing hyphen must be re-trimmed after truncation"
        );
    }

    #[test]
    fn respects_40_char_limit() {
        let out = title_slug(&"ab".repeat(50), ID);
        assert_eq!(out.chars().count(), 40);
    }

    #[test]
    fn digits_survive() {
        assert_eq!(title_slug("Fix issue 1234", ID), "fix-issue-1234");
    }

    #[test]
    fn empty_title_falls_back() {
        assert_eq!(title_slug("", ID), "session-3f2a1b9c");
    }

    // --- 契約 §51.3.3 共有テストベクタの補完 3 本（brief の下限に無いもの） ---

    #[test]
    fn preserves_hyphens_already_present_in_input() {
        // §51.3.3 3 行目。入力に元からあるハイフンが圧縮・除去で潰れないこと。
        assert_eq!(title_slug("re-run tests", ID), "re-run-tests");
    }

    #[test]
    fn truncates_at_exactly_40_chars_dropping_the_rest() {
        // §51.3.3 7 行目。切り詰め境界のもう一方の側: 40 文字ちょうどで切れ、
        // その先の "-c" が丸ごと落ちる（再トリム対象の末尾ハイフンは生まれない）。
        let title = format!("{} c", "b".repeat(40));
        assert_eq!(title_slug(&title, ID), "b".repeat(40));
    }

    #[test]
    fn symbols_only_title_falls_back_to_session_id_prefix() {
        // §51.3.3 9 行目。記号のみの入力も非 ASCII 専用と同じくフォールバックする。
        assert_eq!(title_slug("!!!", ID), "session-3f2a1b9c");
    }
}
