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
    ///
    /// 🔴 レビュー(task-19-review.md §3)で判明: needle 1 本だけでは「定数のバイト列と
    /// needle が同じ区間のものであること」を検査できず、needle が覆っていない領域
    /// (`FIXTURE_PROGRESS` の `[2/3] linking` 側)も自由に動けた。各区間の needle を
    /// 複数へ広げ、`the_fixture_chunks_match_the_script_body` で「定数側」と
    /// 「スクリプト本文側」の両方に現れることを検査する。
    const FIXTURE_CHUNKS: [(&[u8], &[&str]); 5] = [
        (FIXTURE_BANNER, &["fake-generic-cli started"]),
        (FIXTURE_PROGRESS, &["[1/3]", "building", "[2/3]", "linking"]),
        (FIXTURE_PROMPT, &["continue? [y/N]"]),
        (FIXTURE_DONE, &["[3/3]", "done"]),
        (FIXTURE_RESUMED, &["[post]", "resumed after idle"]),
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
    /// 🔴 レビュー(task-19-review.md §3)で判明した不足を反映している:
    /// - `contains` と `printf == 6` の 2 assert だけでは、`FIXTURE_CHUNKS` の要素数にも
    ///   重複にも依らない片方向の検査だったため、行を削る/複製ですり替える変異が生き延びた
    ///   → `len() + 1 == printf_count` と needle の重複チェックを追加(ただしこの
    ///   `len() + 1` は直前の `printf_count == 6` の下で実質 `len() == 5` に縮退しており、
    ///   「区間数を printf 数へ結び付ける」働きはしていない。恒真ではないが、この assert
    ///   単独が捕まえる変異は「スクリプトを変えずに表の行だけ増減する」形のみである)
    /// - needle が定数(バイト列)側と対応していることを測る assert が無く、定数の文言だけ
    ///   差し替える/表の行を入れ替える/needle が覆っていない領域を書き換える、の 3 変異が
    ///   すべて生き延びた → needle を複数へ広げ、各 needle が「定数側」と「スクリプト本文側」
    ///   の両方に現れることを検査する
    #[test]
    fn the_fixture_chunks_match_the_script_body() {
        let body = std::fs::read_to_string(fixture_script_path()).expect("read fixture");

        // `cursor` は「本文のどこまで読んだか」。needle を表の順に前から探し、見つかった
        // 位置より後ろだけを次の needle の探索範囲にする ―― 出現の有無だけを見ると、
        // 印字ブロックを 1 つ別の位置へ移す変異(例: `[3/3] done` を BEL プロンプトより
        // 前へ)が全緑で生き延びる。手動スモークの項目 2/3/5 は印字の順序そのものを
        // 見る手順なので、表は集合ではなく列として固定する
        let mut cursor = 0usize;
        for (chunk, needles) in FIXTURE_CHUNKS {
            let chunk_text = String::from_utf8_lossy(chunk);
            for needle in needles {
                assert!(
                    chunk_text.contains(needle),
                    "FIXTURE_CHUNKS の定数と needle {needle:?} が対応していない"
                );
                assert!(
                    body.contains(needle),
                    "FIXTURE_CHUNKS の区間 {needle:?} がスクリプト本文に見つからない"
                );
                let Some(offset) = body[cursor..].find(needle) else {
                    panic!("FIXTURE_CHUNKS の needle {needle:?} が表の順(= 印字順)に現れない");
                };
                cursor += offset + needle.len();
            }
        }

        // 内訳: banner 1 + progress 2 + prompt 1 + done 1 + resumed 1 = 6
        // (`FIXTURE_PROGRESS` だけが 2 つの `printf` をまとめて模している)。
        // 区間を 1 つ足して定数を足さないと、ここが赤くなる。
        let printf_count = body.matches("printf").count();
        assert_eq!(
            printf_count, 6,
            "printf の出現数が想定と異なる(区間が定数と食い違っている可能性)"
        );

        // `FIXTURE_CHUNKS` の区間数を printf の数へ結び付ける形を意図しているが、直前の
        // `printf_count == 6` により実質 `FIXTURE_CHUNKS.len() == 5` に縮退しており、両辺の
        // 関係式としては働いていない(恒真ではないが「結び付ける」効果は無い。上のコメント
        // 参照)。単独では「スクリプトを変えずに表の行だけ増減する」変異のみ捕まえる。
        assert_eq!(
            FIXTURE_CHUNKS.len() + 1,
            printf_count,
            "FIXTURE_CHUNKS の区間数とスクリプトの printf 数が対応していない"
        );

        // needle が重複していると「1 行を別の行の複製で置き換える」変異が
        // 型注釈も要素数も変えずに(コンパイルエラーにもならずに)通ってしまう
        let mut needles: Vec<&str> = FIXTURE_CHUNKS
            .iter()
            .flat_map(|(_, ns)| ns.iter().copied())
            .collect();
        let unique_count = needles.len();
        needles.sort_unstable();
        needles.dedup();
        assert_eq!(
            needles.len(),
            unique_count,
            "FIXTURE_CHUNKS の needle が重複している"
        );

        // 🔴 レビュー(task-19-review.md 新 Important 3): needle は印字可能テキストしか
        // 覆っておらず、制御バイト(BEL)は定数側・スクリプト側のどちらからも自由に動けた
        // (OSC 終端子の BEL / 本物のプロンプト BEL を落としても上の assert は反応しない)。
        // BEL の個数を両側で数え、リテラル 2(OSC 終端子 1 + プロンプト 1)にも固定する
        // ―― 交差等値だけでは両側から同時に 1 個ずつ消す変異が生き延びるため。
        let script_bels = body.matches("\\007").count();
        let const_bels: usize = FIXTURE_CHUNKS
            .iter()
            .map(|(chunk, _)| chunk.iter().filter(|b| **b == 0x07).count())
            .sum();
        assert_eq!(
            script_bels, 2,
            "スクリプトの BEL は OSC 終端子とプロンプトの 2 つ"
        );
        assert_eq!(
            const_bels, script_bels,
            "定数側とスクリプト側の BEL 数が食い違っている"
        );

        // 🔴 レビュー(task-19-review.md 再レビュー / 修正ラウンド 2 の束ね分): BEL の個数
        // だけでは OSC 列の構造(導入子 `ESC ] 0 ;`)が固定されない。導入子だけを落として
        // BEL を残すと(`printf 'fake-generic-cli started\007\n'`)、上の BEL カウントは
        // 2 のまま変わらないため全緑で生き延びる。導入子が素のベルへ変わると手動スモーク
        // 項目 2(誤検知が起きないこと)の意味が逆転する(契約 §118.5 / RULINGS §14.10)。
        // `body` はスクリプトのソーステキストであり、`printf` の引数に書かれた `\033` は
        // 実行時に解釈される前の文字通り 4 文字のリテラルである(`script_bels` が `\007`
        // を 4 文字として数えているのと同じ理由)。
        assert!(
            body.contains("\\033]0;"),
            "スクリプト本文に OSC 導入子(\\033]0;)が見つからない"
        );
        assert!(
            FIXTURE_BANNER.windows(4).any(|w| w == b"\x1b]0;"),
            "FIXTURE_BANNER に OSC 導入子(ESC ] 0 ;)が無い"
        );
    }

    /// 契約 §118.5: `DONE` の後の待ちは、手動スモークの `silence_timeout_secs`(= 5)より
    /// 長いこと。`sleep 12` → `sleep 1` のように短くすると、沈黙タイムアウトを経由せずに
    /// 出力が再開してしまい、手動スモーク項目 5(`reason == "output_activity"` の機械読み)
    /// がテスト緑のまま静かに観測不能になる(task-19-review.md Important 2)。
    #[test]
    fn the_fixture_waits_longer_than_the_smoke_silence_timeout() {
        const SMOKE_SILENCE_TIMEOUT_SECS: u64 = 5;
        let body = std::fs::read_to_string(fixture_script_path()).expect("read fixture");

        // `[3/3]`(DONE の印字)と `[post]`(再開の印字)で挟まれた区間を切り出す。
        // 秒数の抽出も read の不在検査も、この 1 つの `silence_gap` に対してのみ行う
        // ―― 抽出をこの区間の外(例: ファイル末尾まで)に対して回すと、`sleep` を
        // 再開印字より後ろへ移す変異(M-J。沈黙区間そのものが消える)が緑のまま
        // 生き延びる(task-19-review.md 新 Important 5)。
        let (_, after_done) = body.split_once("[3/3]").expect("`DONE` の行が無い");
        let (silence_gap, _) = after_done.split_once("[post]").expect("再開の印字が無い");

        // 行頭が `sleep ` の最初の行を採る。`while true; do sleep 3600; done` の
        // `sleep 3600` は `silence_gap` の外(`[post]` より後ろ)にあるため対象外。
        let secs: u64 = silence_gap
            .lines()
            .find_map(|l| l.strip_prefix("sleep "))
            .and_then(|s| s.trim().parse().ok())
            .expect("`DONE` と再開印字の間の沈黙 sleep 行が読めない");
        assert!(
            secs > SMOKE_SILENCE_TIMEOUT_SECS,
            "sleep {secs} はスモークの沈黙タイムアウト {SMOKE_SILENCE_TIMEOUT_SECS} 秒以下"
        );

        // `DONE` から再開印字までの区間は、入力を読まずに自力で印字すること
        // (契約 §118.5)。読むと手動スモーク項目 5 が `UserInput` 経路へ戻る。
        assert!(
            !silence_gap.contains("read "),
            "`DONE` から再開印字までの区間で stdin を読んでいる"
        );

        // 区間の中身は `sleep <n>` の 1 行だけであること。始点と終点を錨づけても、
        // 中身の検査が「`read ` が無い」の 1 本だけでは、`[post]` へ到達させない別の
        // 阻害要因(`while true; do sleep 3600; done` を前へ移す / `head -n 1` を挿す)を
        // 差し込めてしまい、スクリプトが `[post] resumed after idle` を永久に印字しない
        // まま全緑になる ―― 手動スモーク項目 5 の唯一の駆動源が静かに消える。
        // 期待値 1 はリテラルであり秒数には依存しない(`sleep 30` へ変えても緑のまま)。
        let gap_lines: Vec<&str> = silence_gap.lines().collect();
        // 先頭は `[3/3]` を含む printf 行の断片、末尾は `[post]` を含む printf 行の断片
        let inner = gap_lines
            .get(1..gap_lines.len().saturating_sub(1))
            .unwrap_or(&[]);
        let effective: Vec<&str> = inner
            .iter()
            .copied()
            .filter(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('#')
            })
            .collect();
        assert_eq!(
            effective.len(),
            1,
            "沈黙区間の実効行は `sleep <n>` の 1 行だけであること (実際: {effective:?})"
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
