#!/usr/bin/env bash
# scripts/measure-perf.sh の純粋関数と、実プロセスを触らない経路だけを検証する。
# アプリの起動は行わない（実機計測は M3-4 Task 15 の人間ゲート）。
#
# 対応づけ（§104.2「対応づけは起動時刻で行う」）の判定は、実システムの ps ではなく
# リテラルの ps テーブルに対して測る。実プロセスに依存するのは
# descendants / rss_mb_of の 4 件だけである。
set -uo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./measure-perf.sh
KAMUX_LIB_ONLY=1 source "$script_dir/measure-perf.sh"

fail=0
asserts=0
# テスト自身が assert の実数を守る。source した先で set -u に当たってブロックが
# 丸ごと走らなかった場合、fail=0 のまま exit 0 になるのを防ぐ。
EXPECTED_ASSERTS=69

ok() { asserts=$((asserts + 1)); printf 'ok   %s\n' "$1"; }
ng() { asserts=$((asserts + 1)); printf 'NG   %s\n' "$1"; fail=1; }
# eq <ラベル> <実際> <期待>
eq() {
  if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 —— 期待 [$3] / 実際 [$2]"; fi
}
# contains <ラベル> <対象> <部分文字列>
contains() {
  case "$2" in
    *"$3"*) ok "$1" ;;
    *) ng "$1 —— [$3] が出力に無い: [$2]" ;;
  esac
}
# lacks <ラベル> <対象> <部分文字列>
lacks() {
  case "$2" in
    *"$3"*) ng "$1 —— [$3] が出力に在る: [$2]" ;;
    *) ok "$1" ;;
  esac
}

# ---------------------------------------------------------------- フィクスチャ
WK_DIR=/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices
# webkit_row <pid> <etime> <rss> <kind>
webkit_row() {
  printf '%s 1 %s %s %s/com.apple.WebKit.%s.xpc/Contents/MacOS/com.apple.WebKit.%s\n' \
    "$1" "$2" "$3" "$WK_DIR" "$4" "$4"
}

# テーブル 1: 他アプリの WebContent が kamux より「前」に立っている（etime が大きい）。
# 片側窓（§104.2 の対応づけ）なら拾わない。abs() の両側窓だと拾って 2 本になる。
# kamux を 1-00:00:00 に置いてあるので etime の dd- 項も同時に効く。
table_foreign="$(
  printf '1000 1 1-00:00:00 105000 /Applications/kamux.app/Contents/MacOS/kamux\n'
  webkit_row 1001 23:59:51 46000 WebContent
  webkit_row 1002 23:59:51 27000 GPU
  webkit_row 1003 23:59:51 14000 Networking
  webkit_row 2001 1-00:00:09 9000 WebContent
)"

# テーブル 2: 同時刻の組が 2 つ（kamux と別アプリ）。それぞれ自分の側だけを拾うこと。
table_two_groups="$(
  printf '1000 1 01:00:00 105000 /Applications/kamux.app/Contents/MacOS/kamux\n'
  webkit_row 1001 00:59:51 46000 WebContent
  webkit_row 1002 00:59:51 27000 GPU
  webkit_row 1003 00:59:51 14000 Networking
  printf '3000 1 00:10:00 80000 /Applications/Other.app/Contents/MacOS/Other\n'
  webkit_row 3001 00:09:51 40000 WebContent
  webkit_row 3002 00:09:51 20000 GPU
  webkit_row 3003 00:09:51 10000 Networking
)"

# テーブル 3: ヘルパが 2 本しか無い（Networking が欠けている）。§104.2 の理由 2。
table_missing="$(
  printf '1000 1 01:00:00 105000 /Applications/kamux.app/Contents/MacOS/kamux\n'
  webkit_row 1001 00:59:51 46000 WebContent
  webkit_row 1002 00:59:51 27000 GPU
)"

# テーブル 4: ヘルパが 1 本も無い。「クリーンだった」と読ませない。
table_none="$(printf '1000 1 01:00:00 105000 /Applications/kamux.app/Contents/MacOS/kamux\n')"

# テーブル 5: PTY 子孫を持つプロセスツリー（参考値 2 の母数）
table_tree="$(
  printf '100 1 00:10:00 10000 /Applications/kamux.app/Contents/MacOS/kamux\n'
  printf '101 100 00:09:00 5000 /bin/zsh\n'
  printf '102 101 00:08:00 90000 /usr/local/bin/claude\n'
  printf '103 100 00:09:00 2000 /usr/libexec/something\n'
)"

# テーブル 6: PTY 子孫を持つ kamux ツリー + WebKit ヘルパ 3 本。
# table_foreign は descendants(1000) = non_pty_pids(1000) = [1000] に縮退していて、
# 参考値 1 と 参考値 2 が同じ値を印字する。取り違えても出力が 1 文字も変わらないので、
# 参考値の配線はそのテーブルの上では観測できない。ここでは
#   参考値 1 = 1000,1004,1005,1006 = 197.3 MB
#   参考値 2 = 1000,1006           = 104.5 MB
#   参考値 3 = 統制対象 ∪ 全子孫    = 282.2 MB
#   K        = 1000               = 102.5 MB
#   統制値   = 1000,1001,1002,1003 = 187.5 MB
# の 5 つがすべて別の値になる。
table_full="$(
  printf '1000 1 1-00:00:00 105000 /Applications/kamux.app/Contents/MacOS/kamux\n'
  webkit_row 1001 23:59:51 46000 WebContent
  webkit_row 1002 23:59:51 27000 GPU
  webkit_row 1003 23:59:51 14000 Networking
  printf '1004 1000 23:59:00 5000 /bin/zsh\n'
  printf '1005 1004 23:58:00 90000 /usr/local/bin/claude\n'
  printf '1006 1000 23:59:00 2000 /usr/libexec/something\n'
  webkit_row 2001 1-00:00:09 9000 WebContent
)"

# ---------------------------------------------------------------- descendants（実プロセス）
sleep 30 &
child=$!
result="$(descendants $$)"
case " $result " in
  *" $child "*) ok "descendants が子プロセス $child を含む" ;;
  *) ng "descendants に $child が無い: $result" ;;
esac
kill "$child" 2>/dev/null || true
wait "$child" 2>/dev/null || true

case " $(descendants $$) " in
  *" $$ "*) ok "descendants が起点 PID を含む" ;;
  *) ng "descendants が起点 PID を含まない" ;;
esac

# ---------------------------------------------------------------- rss_mb_of（実プロセス）
mb="$(rss_mb_of $$)"
if awk -v v="$mb" 'BEGIN { exit !(v + 0 > 0) }'; then
  ok "rss_mb_of が正の値を返す ($mb MB)"
else
  ng "rss_mb_of が 0 以下: $mb"
fi

if [ "$(rss_mb_of)" = "0" ]; then
  ok "rss_mb_of は引数なしで 0"
else
  ng "rss_mb_of の引数なしが 0 でない: $(rss_mb_of)"
fi

# ---------------------------------------------------------------- verdict
if verdict "テスト値" 100 200 "MB" >/dev/null; then
  ok "verdict は上限未満で成功する"
else
  ng "verdict が上限未満で失敗した"
fi
if verdict "テスト値" 300 200 "MB" >/dev/null; then
  ng "verdict が上限超過で成功した"
else
  ok "verdict は上限超過で失敗する"
fi
if verdict "CPU" 0.42 1.0 "%" >/dev/null; then
  ok "verdict が小数を比較できる"
else
  ng "verdict の小数比較が壊れている"
fi
# §0 / §104.2 は「300MB 未満」であって「以下」ではない
if verdict "境界" 200 200 "MB" >/dev/null; then
  ng "verdict が上限ちょうどで成功した（上限未満が正）"
else
  ok "verdict は上限ちょうどで失敗する"
fi
# 空値を 0 と読んで PASS を出さないこと
if verdict "空値" "" 300 "MB" >/dev/null 2>&1; then
  ng "verdict が空の値で成功した"
else
  ok "verdict は空の値で失敗する"
fi

# ---------------------------------------------------------------- etime_to_sec
eq "etime_to_sec 00:05"     "$(etime_to_sec 00:05)"     "5"
eq "etime_to_sec 12:34"     "$(etime_to_sec 12:34)"     "754"
eq "etime_to_sec 01:02:03"  "$(etime_to_sec 01:02:03)"  "3723"
eq "etime_to_sec 3-04:05:06" "$(etime_to_sec 3-04:05:06)" "273906"

# ---------------------------------------------------------------- webkit_pids_for（純関数）
out="$(webkit_pids_for 86400 30 "$table_foreign" 2>/dev/null)"
rc=$?
eq "他アプリの先行ヘルパを拾わない" "$out" "1001 1002 1003"
eq "テーブル 1 は成功する" "$rc" "0"

out="$(webkit_pids_for 3600 30 "$table_two_groups" 2>/dev/null)"
eq "同時刻の組が 2 つあっても kamux の側を拾う" "$out" "1001 1002 1003"
out="$(webkit_pids_for 600 30 "$table_two_groups" 2>/dev/null)"
eq "同じテーブルから別アプリの側も起動時刻で引ける" "$out" "3001 3002 3003"

out="$(webkit_pids_for 3600 30 "$table_missing" 2>/dev/null)"
rc=$?
eq "ヘルパが 2 本なら失敗する" "$rc" "1"
eq "ヘルパが 2 本なら PID を返さない" "$out" ""

out="$(webkit_pids_for 3600 30 "$table_none" 2>/dev/null)"
rc=$?
eq "ヘルパが 0 本なら失敗する（クリーンと読まない）" "$rc" "1"

# ---------------------------------------------------------------- measured_pids（呼び出し側）
out="$(measured_pids 1000 "$table_foreign" 2>/dev/null)"
rc=$?
eq "measured_pids は 4 プロセスを返す" "$out" "1000 1001 1002 1003"
eq "measured_pids はテーブル 1 で成功する" "$rc" "0"

out="$(measured_pids 1000 "$table_missing" 2>/dev/null)"
rc=$?
eq "measured_pids はヘルパ不足で失敗する" "$rc" "1"
eq "measured_pids はヘルパ不足で PID を返さない" "$out" ""

out="$(measured_pids 9999 "$table_foreign" 2>/dev/null)"
rc=$?
eq "measured_pids は未知の root で失敗する" "$rc" "1"

# ---------------------------------------------------------------- descendants / non_pty_pids（純関数）
eq "descendants はテーブルからツリーを組む" "$(descendants 100 "$table_tree")" "100 101 102 103"
eq "non_pty_pids は PTY 子孫を落とす" "$(non_pty_pids 100 "$table_tree")" "100 103"
eq "rss_mb_in はテーブルから RSS を合計する" "$(rss_mb_in "$table_tree" 100 103)" "11.7"

# ---------------------------------------------------------------- cmd_startup
# 実バイナリを起動せずに本番経路を通す。pkill / app_pid / open を差し替える。
# startup_case <perf.log へ書く内容> <バンドルを作るか yes|no>
startup_case() {
  local content="$1" bundle="$2" tmp
  tmp="$(mktemp -d)"
  (
    APP_NAME="kamux-absent-in-test"
    APP_PATH="$tmp/kamux.app"
    PERF_LOG="$tmp/perf.log"
    STARTUP_BUDGET_MS=1000
    STARTUP_WAIT_TICKS=2
    [ "$bundle" = "yes" ] && mkdir -p "$APP_PATH"
    # 稼働中の kamux を殺さないための必須の差し替え
    pkill() { :; }
    app_pid() { printf ''; }
    open() { [ -n "$content" ] && printf '%s' "$content" >>"$PERF_LOG"; return 0; }
    cmd_startup
    printf 'exit_code=%s\n' "$exit_code"
  )
  rm -rf "$tmp"
}

out="$(startup_case '[kamux-perf] rust_setup_ms=120
[kamux-perf] frontend_ready_ms=842
' yes 2>&1)"
contains "起動時間が上限未満なら PASS" "$out" "PASS"
contains "rust_setup_ms の内訳を出す" "$out" "rust_setup_ms=120"
contains "起動時間の PASS で exit_code=0" "$out" "exit_code=0"

out="$(startup_case '[kamux-perf] rust_setup_ms=120
[kamux-perf] frontend_ready_ms=1500
' yes 2>&1)"
contains "起動時間が上限超過なら FAIL" "$out" "FAIL"
contains "起動時間の FAIL で exit_code=1" "$out" "exit_code=1"

# 裁定 112: rust_setup_ms が無い場合も FAIL（? を出して素通りさせない）
out="$(startup_case '[kamux-perf] frontend_ready_ms=842
' yes 2>&1)"
# 「rust_setup_ms」だけを見ると INFO 行の `rust_setup_ms=?` で満たされてしまう。
# FAIL の本文を逐語で見る。
contains "rust_setup_ms 欠落で FAIL" "$out" "FAIL  起動時間          perf.log に rust_setup_ms がありません"
lacks "rust_setup_ms 欠落で PASS を出さない" "$out" "PASS"
contains "rust_setup_ms 欠落で exit_code=1" "$out" "exit_code=1"

out="$(startup_case '[kamux-perf] rust_setup_ms=120
' yes 2>&1)"
contains "frontend_ready_ms 欠落で FAIL" "$out" "frontend_ready_ms"
contains "frontend_ready_ms 欠落で exit_code=1" "$out" "exit_code=1"

out="$(startup_case '' no 2>&1)"
contains "バンドルが無ければ FAIL" "$out" "アプリバンドルがありません"
contains "バンドルが無ければ exit_code=1" "$out" "exit_code=1"
contains "バンドルが無いときのビルド手順は npm" "$out" "npm run tauri build"

# perf.log は追記される（§Task 13 の record_to）。最後の行を読むこと。
out="$(startup_case '[kamux-perf] rust_setup_ms=999
[kamux-perf] frontend_ready_ms=1500
[kamux-perf] rust_setup_ms=120
[kamux-perf] frontend_ready_ms=842
' yes 2>&1)"
contains "追記されたログの最後の値で判定する" "$out" "PASS"
contains "追記されたログの最後の内訳を出す" "$out" "rust_setup_ms=120"

# ---------------------------------------------------------------- cmd_memory
# memory_case <table>
ps_call_log="$(mktemp)"
memory_case() {
  # cmd_memory 側の `local table` と名前が衝突すると bash の動的スコープで
  # 未割り当ての変数を掴む。テスト側は別名にする。
  local snapshot_table="$1"
  : >"$ps_call_log"
  (
    ps_snapshot() { printf 'call\n' >>"$ps_call_log"; printf '%s\n' "$snapshot_table"; }
    app_pid() { printf '1000\n'; }
    cmd_memory
    printf 'exit_code=%s\n' "$exit_code"
  )
}

out="$(memory_case "$table_foreign" 2>&1)"
contains "メモリの統制値は 4 プロセスの総和" "$out" "187.5"
contains "メモリが上限未満なら PASS" "$out" "PASS"
contains "メモリの PASS で exit_code=0" "$out" "exit_code=0"
contains "メモリは参考値も併記する" "$out" "参考"
# 「ps を 1 回だけ打って 1 つのスナップショットから全部導出する」を観測する。
# 2 回打つと etime が秒単位でずれ、対応づけが静かに崩れる。
eq "cmd_memory は ps を 1 回だけ打つ" "$(wc -l <"$ps_call_log" | tr -d ' ')" "1"

# 参考値 1 / 2 / 3 と K は、ラベルと値をまたぐ 1 本の部分文字列で見る。
# 値だけを見ると、参考値 1 と 参考値 2 の集合を取り違えても両方の数が出力に残るので
# 緑のままになる（§104.2 の参考値はラベルと値が対応していて初めて意味を持つ）。
out="$(memory_case "$table_full" 2>&1)"
contains "参考値 1 は kamux ツリー全体（PTY 込み）" "$out" \
  "参考値 1: kamux ツリー（PTY 込み。WebKit ヘルパは ppid=1 のため含まない） 197.3 MB"
contains "参考値 2 は PTY 子孫を落とした値" "$out" \
  "参考値 2: kamux ツリー − PTY 子孫（同上） 104.5 MB"
# §104.2「ただし子孫を含めた合計値も参考値として必ず併記する」
contains "参考値 3 は子孫を含めた合計値" "$out" \
  "参考値 3: 統制対象 4 プロセス + 全子孫（§104.2「子孫を含めた合計値」） 282.2 MB"
contains "kamux 本体単体の RSS を内訳として出す" "$out" "内訳 kamux 本体単体: 102.5 MB"
contains "PTY 子孫があっても統制値は 4 プロセスの総和のまま" "$out" "187.5MB (上限 300MB)"
contains "PTY 子孫を持つツリーでもメモリは PASS" "$out" "exit_code=0"

out="$(memory_case "$table_missing" 2>&1)"
contains "ヘルパ不足なら メモリは FAIL" "$out" "FAIL"
lacks "ヘルパ不足なら メモリの PASS を出さない" "$out" "PASS"
contains "ヘルパ不足なら メモリで exit_code=1" "$out" "exit_code=1"

# ---------------------------------------------------------------- cmd_idle_cpu
# 偽 top の出力は本番のパイプ（top … | awk）に飲まれて標準出力へ出ない。
# 「呼ばれたか」はファイルへ記録して観測する。
top_marker="$(mktemp)"
out="$(
  (
    ps_snapshot() { printf '%s\n' "$table_missing"; }
    app_pid() { printf '1000\n'; }
    top() { printf 'TOP-CALLED\n' >>"$top_marker"; }
    cmd_idle_cpu
    printf 'exit_code=%s\n' "$exit_code"
  ) 2>&1
)"
contains "ヘルパ不足なら アイドル CPU は FAIL" "$out" "FAIL"
eq "ヘルパ不足なら top を走らせない" "$(cat "$top_marker")" ""
contains "ヘルパ不足なら アイドル CPU で exit_code=1" "$out" "exit_code=1"
rm -f "$top_marker"

# ---------------------------------------------------------------- cmd_idle_cpu（成功経路）
# top をシェル関数で差し替えて成功経路を丸ごと通す（関数探索は PATH に優先する）。
# 偽 top は -l の値を無視して常に 3 ブロック出すので、平均・最大は
# 「1 ブロック目を捨てているか」だけで決まる。捨てなければ 1 ブロック目の
# 400.00 が混ざって mean=133.67 / peak=400.00 になる。
# idle_block <1000 の %CPU> <1001> <1002> <1003>
idle_block() {
  printf 'Processes: 512 total, 3 running, 509 sleeping, 2400 threads\n'
  printf '2026/08/25 06:00:00\n'
  printf 'Load Avg: 1.50, 1.60, 1.70\n'
  printf 'CPU usage: 2.10%% user, 3.20%% sys, 94.70%% idle\n'
  printf 'PhysMem: 16G used (2000M wired), 1000M unused.\n'
  printf '\n'
  printf 'PID    %%CPU\n'
  printf '1000   %s\n' "$1"
  printf '1001   %s\n' "$2"
  printf '1002   %s\n' "$3"
  printf '1003   %s\n' "$4"
}
idle_top_args="$(mktemp)"
# idle_case <block2 の 4 値> <block3 の 4 値>（各引数は空白区切りの 4 個）
idle_case() {
  local b2="$1" b3="$2"
  : >"$idle_top_args"
  (
    ps_snapshot() { printf '%s\n' "$table_foreign"; }
    app_pid() { printf '1000\n'; }
    top() {
      # 呼び出しごとに 1 行追記する。2 回走ったら行数が増えて eq が落ちる。
      printf '%s\n' "$*" >>"$idle_top_args"
      idle_block 100.00 100.00 100.00 100.00
      # shellcheck disable=SC2086
      idle_block $b2
      # shellcheck disable=SC2086
      idle_block $b3
    }
    cmd_idle_cpu
    printf 'exit_code=%s\n' "$exit_code"
  ) 2>&1
}

# ケース A: block2 = 0.25 / block3 = 0.75 → mean 0.50 / peak 0.75。両方 PASS。
out="$(idle_case "0.10 0.05 0.05 0.05" "0.30 0.20 0.15 0.10")"
contains "アイドル CPU 平均は 1 ブロック目を捨てて平均する" "$out" "PASS  アイドルCPU平均 0.50% (上限 1.0%)"
contains "アイドル CPU 最大は 2 ブロック目以降の最大" "$out" "PASS  アイドルCPU最大 0.75% (上限 5.0%)"
contains "アイドル CPU が両方上限未満なら exit_code=0" "$out" "exit_code=0"
# §104.3「`top -l 61 -s 1 -stats pid,cpu -pid …` を 1 回だけ走らせ」。
# -pid は統制対象 4 本ぶん渡ること（§104.2 理由 2「2 本しか挙げない手順は数え漏らす」）。
eq "top は §104.3 の逐語の引数で 1 回だけ走る" "$(cat "$idle_top_args")" \
  "-l 61 -s 1 -stats pid,cpu -pid 1000 -pid 1001 -pid 1002 -pid 1003"
# §104.3 の前提。Task 15 の実施者に「操作しない」だけでは足りないことを伝える
contains "アイドル CPU は §104.3 の 30 秒前提を出す" "$out" "出力が止まって 30 秒以上経過した状態で"

# ケース B: block2 = 2.50 / block3 = 3.50 → mean 3.00 / peak 3.50。
# 平均は上限 1.0 を超えて FAIL、最大は上限 5.0 未満で PASS。
# 平均の判定に PEAK_MAX(5.0) を渡すと 3.00% が偽 PASS になる（§104.3 は平均 < 1.0%）。
out="$(idle_case "1.00 0.60 0.50 0.40" "1.50 1.00 0.60 0.40")"
contains "平均の上限は 1.0% であり 3.00% は FAIL" "$out" "FAIL  アイドルCPU平均 3.00% (上限 1.0%)"
contains "最大の上限は 5.0% であり 3.50% は PASS" "$out" "PASS  アイドルCPU最大 3.50% (上限 5.0%)"
contains "アイドル CPU 平均の超過で exit_code=1" "$out" "exit_code=1"
rm -f "$idle_top_args"

# ---------------------------------------------------------------- 前提プロトコル
# Task 15 の実施者はスクリプトのヘッダしか読まない。§104.3 の前提
# 「出力が止まって 30 秒以上経過した状態で」がそこに無いと、実施者は
# 「操作しない」だけを満たして測ってしまう。
eq "前提プロトコルに §104.3 の 30 秒前提が在る" \
  "$(grep -c 'idle-cpu は出力が止まって 30 秒以上経過した状態で実行する' "$script_dir/measure-perf.sh")" "1"

# ---------------------------------------------------------------- assert 件数の検算
if [ "$asserts" -ne "$EXPECTED_ASSERTS" ]; then
  printf 'NG   assert の実数が %s。期待 %s（ブロックが黙って飛んでいる）\n' "$asserts" "$EXPECTED_ASSERTS"
  fail=1
else
  printf 'ok   assert %s 件すべてを実行した\n' "$asserts"
fi

exit "$fail"
