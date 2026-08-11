//! PTY 出力バイト列から「本物の BEL」だけを数えるチャンク跨ぎスキャナ。
//!
//! 素朴に `0x07` を検索してはならない。`ESC ] 0 ; title BEL`（ウィンドウタイトル設定）は
//! シェルのプロンプトが毎回吐く最頻出シーケンスであり、その BEL は終端子であってベルではない。
//! OSC / DCS / SOS / PM / APC の文字列中に現れる `0x07` を除外する。
//!
//! 8 bit C1 制御（`0x9D` 等）は扱わない。UTF-8 ストリーム中では継続バイトと区別できず、
//! 実運用のターミナルは 7 bit 表現を使うため。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ScanState {
    /// 通常出力中。ここでの `0x07` は本物のベル
    #[default]
    Ground,
    /// `ESC` を読んだ直後
    Esc,
    /// OSC / DCS / SOS / PM / APC のペイロード中
    StringMode,
    /// 文字列モード中に `ESC` を読んだ直後（ST 候補）
    StringModeEsc,
}

/// チャンクを跨いで状態を保つ BEL カウンタ。
/// 1 PTY サーフェスの読み取りスレッドが 1 個所有する（共有しない）。
#[derive(Debug, Clone, Default)]
pub struct BelScanner {
    state: ScanState,
}

impl BelScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// チャンクを走査し、含まれる本物の BEL の個数を返す。
    pub fn scan(&mut self, chunk: &[u8]) -> usize {
        let mut count = 0usize;
        let mut i = 0usize;

        while i < chunk.len() {
            if matches!(self.state, ScanState::Ground) {
                // Ground では 0x07 / 0x1B 以外は状態を変えない。SIMD で読み飛ばす
                match memchr::memchr2(0x07, 0x1B, &chunk[i..]) {
                    Some(off) => i += off,
                    None => return count,
                }
            }

            let b = chunk[i];
            i += 1;

            self.state = match (self.state, b) {
                (ScanState::Ground, 0x07) => {
                    count += 1;
                    ScanState::Ground
                }
                (ScanState::Ground, 0x1B) => ScanState::Esc,
                (ScanState::Ground, _) => ScanState::Ground,

                // OSC / DCS / SOS / PM / APC の開始
                (ScanState::Esc, b']' | b'P' | b'X' | b'^' | b'_') => ScanState::StringMode,
                (ScanState::Esc, 0x1B) => ScanState::Esc,
                (ScanState::Esc, 0x07) => {
                    count += 1;
                    ScanState::Ground
                }
                // CSI（ESC [）を含むその他はここで Ground へ戻る
                (ScanState::Esc, _) => ScanState::Ground,

                // 文字列モード中の BEL は終端子。数えない
                (ScanState::StringMode, 0x07) => ScanState::Ground,
                (ScanState::StringMode, 0x1B) => ScanState::StringModeEsc,
                (ScanState::StringMode, _) => ScanState::StringMode,

                // ST（ESC \）で文字列モードを抜ける
                (ScanState::StringModeEsc, b'\\') => ScanState::Ground,
                (ScanState::StringModeEsc, 0x1B) => ScanState::StringModeEsc,
                // 不正シーケンス。迷ったら数えない方向へ倒し、文字列モードに留まる
                (ScanState::StringModeEsc, _) => ScanState::StringMode,
            };
        }

        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(input: &[u8]) -> usize {
        BelScanner::new().scan(input)
    }

    #[test]
    fn counts_a_bare_bel() {
        assert_eq!(count(b"hello\x07world"), 1);
    }

    #[test]
    fn counts_multiple_bare_bels() {
        assert_eq!(count(b"\x07\x07\x07"), 3);
    }

    #[test]
    fn plain_text_has_no_bel() {
        assert_eq!(count(b"just some ordinary output\n"), 0);
    }

    #[test]
    fn empty_chunk_has_no_bel() {
        assert_eq!(count(b""), 0);
    }

    #[test]
    fn osc_terminator_bel_is_not_a_bell() {
        // ウィンドウタイトル設定。シェルのプロンプトが毎回吐く最頻出シーケンス
        assert_eq!(count(b"\x1b]0;my-title\x07"), 0);
    }

    #[test]
    fn osc_terminated_by_st_is_not_a_bell() {
        assert_eq!(count(b"\x1b]0;my-title\x1b\\"), 0);
    }

    #[test]
    fn bel_after_osc_is_counted() {
        assert_eq!(count(b"\x1b]0;t\x07text\x07"), 1);
    }

    #[test]
    fn bel_inside_dcs_payload_is_not_counted() {
        assert_eq!(count(b"\x1bPpayload\x07more\x1b\\"), 0);
    }

    #[test]
    fn bel_inside_apc_and_pm_and_sos_is_not_counted() {
        assert_eq!(count(b"\x1b_apc\x07\x1b\\"), 0);
        assert_eq!(count(b"\x1b^pm\x07\x1b\\"), 0);
        assert_eq!(count(b"\x1bXsos\x07\x1b\\"), 0);
    }

    #[test]
    fn csi_sequences_do_not_open_string_mode() {
        // ESC [ は文字列モードではない。その後の BEL は本物
        assert_eq!(count(b"\x1b[31mred\x1b[0m\x07"), 1);
    }

    #[test]
    fn esc_followed_by_bel_is_counted() {
        assert_eq!(count(b"\x1b\x07"), 1);
    }

    #[test]
    fn state_carries_across_chunk_boundary_inside_osc() {
        let mut s = BelScanner::new();
        assert_eq!(s.scan(b"\x1b]0;par"), 0);
        assert_eq!(s.scan(b"tial-title\x07"), 0); // 終端子。ベルではない
        assert_eq!(s.scan(b"\x07"), 1); // Ground に戻ったので本物
    }

    #[test]
    fn esc_split_across_chunk_boundary_opens_string_mode() {
        let mut s = BelScanner::new();
        assert_eq!(s.scan(b"text\x1b"), 0);
        assert_eq!(s.scan(b"]0;t\x07"), 0);
    }

    #[test]
    fn st_split_across_chunk_boundary_closes_string_mode() {
        let mut s = BelScanner::new();
        assert_eq!(s.scan(b"\x1b]0;t\x1b"), 0);
        assert_eq!(s.scan(b"\\\x07"), 1);
    }

    #[test]
    fn stray_esc_inside_string_mode_stays_in_string_mode() {
        // 迷ったら数えない方向へ倒す（設計 §4.2）
        assert_eq!(count(b"\x1b]0;a\x1bZb\x07"), 0);
    }

    #[test]
    fn consecutive_escapes_do_not_break_the_scanner() {
        assert_eq!(count(b"\x1b\x1b\x1b]0;t\x07"), 0);
        assert_eq!(count(b"\x1b\x1b]0;t\x07"), 0); // ESC 偶数 → (Esc, 0x1B) の自己ループを検出
        assert_eq!(count(b"\x1b]0;t\x1b\x1b\\\x07"), 1); // StringModeEsc 中の ESC 連続 → (StringModeEsc, 0x1B) を検出
    }

    #[test]
    fn large_plain_chunk_is_handled() {
        let mut chunk = vec![b'a'; 8192];
        chunk[4000] = 0x07;
        assert_eq!(count(&chunk), 1);
    }
}
