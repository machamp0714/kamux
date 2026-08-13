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

#[cfg(test)]
mod fixture_tests {
    use super::*;

    /// `tests/fixtures/fake-generic-cli.sh` が実際に吐くバイト列
    const FIXTURE_BANNER: &[u8] = b"\x1b]0;fake-generic-cli\x07fake-generic-cli started\n";
    const FIXTURE_PROGRESS: &[u8] =
        b"\x1b[32m[1/3]\x1b[0m building\n\x1b[32m[2/3]\x1b[0m linking\n";
    const FIXTURE_PROMPT: &[u8] = b"continue? [y/N] \x07";
    const FIXTURE_DONE: &[u8] = b"\x1b[32m[3/3]\x1b[0m done\n";
    /// 契約 §118.5: `DONE` の後、沈黙タイムアウト(スモークの設定は 5 秒)より
    /// 長く待ってから、入力を読まずに印字する区間。手動スモーク項目 5 は
    /// この区間が無いと実施できない(`OutputActivity` を人間の打鍵なしで駆動する
    /// 唯一の手段である)。**BEL を含めないこと** —— 含めると項目 5 の観測に 🟡 が混ざる
    const FIXTURE_RESUMED: &[u8] = b"\x1b[32m[post]\x1b[0m resumed after idle\n";

    /// 定数 → スクリプト本文の対応表。契約 §118.5 の「定数を持たない区間があると
    /// the_whole_fixture_session_yields_exactly_one_bell が網羅を偽って主張する」を
    /// 実際に赤くするための表である(コメントだけでは担保にならない)。
    const FIXTURE_CHUNKS: [(&[u8], &str); 5] = [
        (FIXTURE_BANNER, "fake-generic-cli started"),
        (FIXTURE_PROGRESS, "[1/3]"),
        (FIXTURE_PROMPT, "continue? [y/N]"),
        (FIXTURE_DONE, "[3/3]"),
        (FIXTURE_RESUMED, "resumed after idle"),
    ];

    fn fixture_script_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-generic-cli.sh")
    }

    #[test]
    fn the_fixture_banner_contains_no_bell() {
        // OSC のタイトル設定だけ。ここで反応したら手順 2 のスモークが落ちる
        assert_eq!(BelScanner::new().scan(FIXTURE_BANNER), 0);
    }

    #[test]
    fn the_fixture_progress_lines_contain_no_bell() {
        assert_eq!(BelScanner::new().scan(FIXTURE_PROGRESS), 0);
    }

    #[test]
    fn the_fixture_prompt_contains_exactly_one_bell() {
        assert_eq!(BelScanner::new().scan(FIXTURE_PROMPT), 1);
    }

    /// 契約 §118.5: 沈黙後の再開区間も BEL を持たない。
    /// 持つと手動スモーク項目 5 の観測(`reason == "output_activity"`)に 🟡 が混ざる
    #[test]
    fn the_fixture_resumed_line_contains_no_bell() {
        assert_eq!(BelScanner::new().scan(FIXTURE_RESUMED), 0);
    }

    /// 🔴 期待値 1 は `FIXTURE_RESUMED` を足しても動かない。動いたら区間の中身が誤っている
    /// (契約 §118.5)。配列は手書きであり、定数を足したら必ず `FIXTURE_CHUNKS` へも足すこと
    /// —— 定数を持たない区間があると、この名前が網羅を偽って主張する
    #[test]
    fn the_whole_fixture_session_yields_exactly_one_bell() {
        let mut scanner = BelScanner::new();
        let total: usize = FIXTURE_CHUNKS
            .iter()
            .map(|(chunk, _)| scanner.scan(chunk))
            .sum();
        assert_eq!(total, 1);
    }

    /// 欠陥 4 の手当て: `FIXTURE_CHUNKS` の各定数が実際のスクリプト本文と対応していることを
    /// 機械的に固定する。この assert が無いと、`FIXTURE_CHUNKS` から要素を落としても
    /// `the_whole_fixture_session_yields_exactly_one_bell` は緑のまま通ってしまう。
    ///
    /// 🔴 `contains` と `printf == 6` の 2 assert だけでは不十分だったことが変異検証で
    /// 判明した(片方向の検査であり、`FIXTURE_CHUNKS` の要素数にも重複にも依らないため)。
    /// `len() + 1 == printf_count` と needle の重複チェックを追加している。
    #[test]
    fn the_fixture_chunks_match_the_script_body() {
        let body = std::fs::read_to_string(fixture_script_path()).expect("read fixture");

        for (_, needle) in FIXTURE_CHUNKS {
            assert!(
                body.contains(needle),
                "FIXTURE_CHUNKS の区間 {needle:?} がスクリプト本文に見つからない"
            );
        }

        // 内訳: banner 1 + progress 2 + prompt 1 + done 1 + resumed 1 = 6
        // (`FIXTURE_PROGRESS` だけが 2 つの `printf` をまとめて模している)。
        // 区間を 1 つ足して定数を足さないと、ここが赤くなる。
        let printf_count = body.matches("printf").count();
        assert_eq!(
            printf_count, 6,
            "printf の出現数が想定と異なる(区間が定数と食い違っている可能性)"
        );

        // `FIXTURE_CHUNKS` の区間数を printf の数へ結び付ける。これが無いと配列から
        // 行を落とす変異(型注釈も要素数に合わせて直す)が緑で生き延びる
        // (printf の出現数はスクリプト側のカウントで、配列の要素数には依らないため)。
        // +1 は `FIXTURE_PROGRESS` だけが 2 つの printf を束ねている分。
        assert_eq!(
            FIXTURE_CHUNKS.len() + 1,
            printf_count,
            "FIXTURE_CHUNKS の区間数とスクリプトの printf 数が対応していない"
        );

        // needle が重複していると「1 行を別の行の複製で置き換える」変異が
        // 型注釈も要素数も変えずに(コンパイルエラーにもならずに)通ってしまう
        let mut needles: Vec<&str> = FIXTURE_CHUNKS.iter().map(|(_, n)| *n).collect();
        let unique_count = needles.len();
        needles.sort_unstable();
        needles.dedup();
        assert_eq!(
            needles.len(),
            unique_count,
            "FIXTURE_CHUNKS の needle が重複している"
        );
    }

    #[test]
    fn the_fixture_script_exists_and_is_executable() {
        use std::os::unix::fs::PermissionsExt;
        let path = fixture_script_path();
        let meta = std::fs::metadata(&path)
            .unwrap_or_else(|e| panic!("{} が読めない: {e}", path.display()));
        assert!(
            meta.permissions().mode() & 0o111 != 0,
            "実行ビットが立っていない"
        );
    }

    #[test]
    fn the_fixture_script_does_not_use_hooks() {
        // 汎用 CLI 役なので kamux-relay を呼んではいけない(fake-agent.sh との違い)
        let path = fixture_script_path();
        let body = std::fs::read_to_string(&path).expect("read fixture");
        assert!(
            !body.contains("kamux-relay"),
            "汎用 CLI 役が relay を叩いている"
        );
        assert!(
            !body.contains("KAMUX_SESSION_ID"),
            "汎用 CLI 役が session id を参照している"
        );
    }
}
