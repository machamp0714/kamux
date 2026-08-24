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
EXPECTED_ASSERTS=54

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

# ---------------------------------------------------------------- assert 件数の検算
if [ "$asserts" -ne "$EXPECTED_ASSERTS" ]; then
  printf 'NG   assert の実数が %s。期待 %s（ブロックが黙って飛んでいる）\n' "$asserts" "$EXPECTED_ASSERTS"
  fail=1
else
  printf 'ok   assert %s 件すべてを実行した\n' "$asserts"
fi

exit "$fail"
