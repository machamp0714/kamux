#!/usr/bin/env bash
# kamux 軽量性計測（契約 §0 / 数え方と閾値の正典は §104.2 / §104.3）
#
# 前提プロトコル（守らないと数値が信用できない）:
#   1. memory / idle-cpu はセッションを 5 個表示した状態で実行する
#   2. startup は kamux を一度終了してから実行する（このスクリプトが自動で終了させる）
#   3. 他の WebKit ベースのアプリを終了しておくことは必須ではない（§104.6 の 2）。
#      統制対象の対応づけは起動時刻で行うので、他アプリのヘルパとは区別できる。
#
# 統制値は §104.2 の 4 プロセス（kamux 本体 + com.apple.WebKit.WebContent / .GPU /
# .Networking）の RSS の総和である。判定はこの 4 プロセスだけで下し、子孫を含めた
# 合計値は参考値として併記する。
set -uo pipefail

APP_NAME="${KAMUX_APP_NAME:-kamux}"
# `npm run tauri build` の成果物は Launch Services に登録されていないので、
# `open -a kamux` では見つからない。バンドルのパスを直接開く。
# APP_NAME は tauri.conf.json の productName と一致していること（pgrep -x が依存する）
APP_PATH="${KAMUX_APP_PATH:-src-tauri/target/release/bundle/macos/${APP_NAME}.app}"
PERF_LOG="${KAMUX_PERF_LOG:-$HOME/Library/Application Support/kamux/perf.log}"
STARTUP_BUDGET_MS="${KAMUX_STARTUP_BUDGET_MS:-1000}"
MEM_BUDGET_MB="${KAMUX_MEM_BUDGET_MB:-300}"
IDLE_CPU_MEAN_MAX="${KAMUX_IDLE_CPU_MEAN_MAX:-1.0}"
IDLE_CPU_PEAK_MAX="${KAMUX_IDLE_CPU_PEAK_MAX:-5.0}"
IDLE_SAMPLES="${KAMUX_IDLE_SAMPLES:-60}"
# PTY の中で走る子プロセス。kamux の軽量性の責任範囲外なので統制値から除外する
PTY_CMD_RE="${KAMUX_PTY_CMD_RE:-^(claude|codex|nvim|vim|zsh|bash|sh|node|python3)$}"
# WebKit ヘルパの起動時刻の許容差。実測での差は 9 秒（§104.2）
WEBKIT_TOLERANCE_SEC="${KAMUX_WEBKIT_TOLERANCE_SEC:-30}"
# frontend_ready_ms を待つ回数（1 回 0.2 秒）
STARTUP_WAIT_TICKS="${KAMUX_STARTUP_WAIT_TICKS:-150}"

exit_code=0

# ---------------------------------------------------------------- ヘルパ

# ps を 1 回だけ打って 1 つのスナップショットを作る。列は pid ppid etime rss comm。
# etime を別呼び出しで取ると秒単位でずれるので、対応づけは必ずこの 1 枚から導く。
ps_snapshot() { ps -Ao pid=,ppid=,etime=,rss=,comm=; }

app_pid() { pgrep -x "$APP_NAME" | head -n1; }

# PID 群を昇順に並べて空白区切りの 1 行にする
sort_pids() {
  if [ "$#" -eq 0 ]; then printf '\n'; return; fi
  printf '%s\n' "$@" | sort -n | tr '\n' ' ' | sed 's/ *$//'
  printf '\n'
}

# etime（[[dd-]hh:]mm:ss）を秒へ変換する。
# ps の lstart はロケール依存（この機体では「火  8/25 05:42:45 2026」）で date へ
# 渡せない。etimes は macOS に無い。etime はロケール非依存で外部依存も要らない。
etime_to_sec() {
  printf '%s\n' "$1" | awk -F: '{
    d = 0; h = 0; m = 0; s = 0
    if (NF == 3) {
      h = $1; m = $2; s = $3
      if (index(h, "-") > 0) { split(h, a, "-"); d = a[1]; h = a[2] }
    } else if (NF == 2) {
      m = $1; s = $2
    } else {
      s = $1
    }
    printf "%d\n", d * 86400 + h * 3600 + m * 60 + s
  }'
}

# 起点 PID とその全子孫を昇順の空白区切りで返す（起点自身を含む）
# descendants <root> [table]
descendants() {
  local root="$1" table="${2:-}"
  [ -n "$table" ] || table="$(ps_snapshot)"
  local frontier="$root" all="$root" next child c p
  while [ -n "$frontier" ]; do
    next=""
    for p in $frontier; do
      child="$(printf '%s\n' "$table" | awk -v pp="$p" '$2 == pp { print $1 }')"
      for c in $child; do
        case " $all " in
          *" $c "*) ;;
          *) all="$all $c"; next="$next $c" ;;
        esac
      done
    done
    frontier="$next"
  done
  # shellcheck disable=SC2086
  sort_pids $all
}

# 子孫のうち PTY 子プロセスとその子孫を落としたもの（参考値 2 の母数）
# non_pty_pids <root> [table]
non_pty_pids() {
  local root="$1" table="${2:-}"
  [ -n "$table" ] || table="$(ps_snapshot)"
  local tree pty_all="" kept="" p q comm
  tree="$(descendants "$root" "$table")"
  for p in $tree; do
    comm="$(printf '%s\n' "$table" | awk -v x="$p" '$1 == x { n = split($NF, seg, "/"); print seg[n] }')"
    if printf '%s' "$comm" | grep -Eq "$PTY_CMD_RE"; then
      for q in $(descendants "$p" "$table"); do
        case " $pty_all " in *" $q "*) ;; *) pty_all="$pty_all $q" ;; esac
      done
    fi
  done
  for p in $tree; do
    case " $pty_all " in
      *" $p "*) ;;
      *) kept="$kept $p" ;;
    esac
  done
  # shellcheck disable=SC2086
  sort_pids $kept
}

# スナップショットから PID 群の RSS 合計を MB で返す（純関数）
# rss_mb_in <table> <pid>...
rss_mb_in() {
  local table="$1"
  shift
  if [ "$#" -eq 0 ]; then printf '0\n'; return; fi
  # awk -v は改行を含む値を受け取れないので空白区切りで渡す
  local list="$*"
  printf '%s\n' "$table" | awk -v pids="$list" '
    BEGIN { n = split(pids, a, " "); for (i = 1; i <= n; i++) want[a[i]] = 1 }
    ($1 in want) { s += $4 }
    END { printf "%.1f\n", s / 1024 }'
}

# 渡した PID 群の RSS 合計を MB で返す（実システムを 1 回読む入口）
rss_mb_of() {
  if [ "$#" -eq 0 ]; then printf '0\n'; return; fi
  rss_mb_in "$(ps_snapshot)" "$@"
}

# kamux に属する WebKit ヘルパ 3 本を起動時刻で特定する（純関数）
# webkit_pids_for <kamux_etime_sec> <tolerance_sec> <table>
# 3 本（WebContent / GPU / Networking）が各 1 本そろわなければ 1 を返し、PID を返さない。
# §104.2 の理由 2:「3 本は同時に立つ。2 本しか挙げない手順は数え漏らす。」
webkit_pids_for() {
  local kamux_sec="$1" tol="$2" table="$3"
  local kind rows pid etime sec diff hits count found="" bad=0
  for kind in WebContent GPU Networking; do
    rows="$(printf '%s\n' "$table" | awk -v k="com.apple.WebKit.$kind" '
      { n = split($NF, seg, "/"); if (seg[n] == k) print $1, $3 }')"
    hits=""
    count=0
    while read -r pid etime; do
      [ -n "$pid" ] || continue
      sec="$(etime_to_sec "$etime")"
      # 片側窓。ヘルパは kamux が spawn するので kamux より後に立つ（= etime が小さい）。
      # abs() の両側窓にすると kamux より前に立った他アプリのヘルパを拾う。
      diff=$((kamux_sec - sec))
      if [ "$diff" -ge 0 ] && [ "$diff" -le "$tol" ]; then
        hits="$hits $pid"
        count=$((count + 1))
      fi
    done <<EOF
$rows
EOF
    if [ "$count" -ne 1 ]; then
      printf 'FAIL  com.apple.WebKit.%s が %s 本（1 本ちょうどであること）\n' "$kind" "$count" >&2
      bad=1
    fi
    found="$found $hits"
  done
  if [ "$bad" -ne 0 ]; then
    printf 'FAIL  WebKit ヘルパ 3 本（WebContent / GPU / Networking）がそろいません。0 本を「クリーンだった」と読まないこと\n' >&2
    return 1
  fi
  # shellcheck disable=SC2086
  sort_pids $found
}

# 統制対象の PID 群（§104.2 の 4 プロセス）。ps を 1 回だけ打って純関数へ渡す。
# measured_pids <root> [table]
measured_pids() {
  local root="$1" table="${2:-}"
  [ -n "$table" ] || table="$(ps_snapshot)"
  local etime sec helpers
  etime="$(printf '%s\n' "$table" | awk -v r="$root" '$1 == r { print $3 }')"
  if [ -z "$etime" ]; then
    printf 'FAIL  PID %s が ps のスナップショットに居ません\n' "$root" >&2
    return 1
  fi
  sec="$(etime_to_sec "$etime")"
  helpers="$(webkit_pids_for "$sec" "$WEBKIT_TOLERANCE_SEC" "$table")" || return 1
  # shellcheck disable=SC2086
  sort_pids $root $helpers
}

# 判定を 1 行出力し、超過なら 1 を返す。上限は「未満」であって「以下」ではない（§0 / §104.2）
verdict() {
  local label="$1" value="$2" budget="$3" unit="$4"
  if [ -z "$value" ] || [ -z "$budget" ]; then
    printf 'FAIL  %-16s 値が空です（測れていない）\n' "$label"
    return 1
  fi
  if awk -v v="$value" -v b="$budget" 'BEGIN { exit !(v + 0 < b + 0) }'; then
    printf 'PASS  %-16s %s%s (上限 %s%s)\n' "$label" "$value" "$unit" "$budget" "$unit"
    return 0
  fi
  printf 'FAIL  %-16s %s%s (上限 %s%s)\n' "$label" "$value" "$unit" "$budget" "$unit"
  return 1
}

# ---------------------------------------------------------------- サブコマンド

cmd_startup() {
  printf '== 起動時間 ==\n'
  printf 'INFO: %s を一度終了します\n' "$APP_NAME"
  pkill -x "$APP_NAME" 2>/dev/null || true
  local waited=0
  while [ -n "$(app_pid)" ] && [ "$waited" -lt 50 ]; do sleep 0.2; waited=$((waited + 1)); done

  if [ ! -d "$APP_PATH" ]; then
    printf 'FAIL  起動時間          アプリバンドルがありません: %s\n' "$APP_PATH"
    printf 'INFO  先に `npm run tauri build` を実行するか KAMUX_APP_PATH を指定してください\n'
    exit_code=1
    return
  fi

  mkdir -p "$(dirname "$PERF_LOG")"
  : >"$PERF_LOG"
  open "$APP_PATH"

  waited=0
  while [ "$waited" -lt "$STARTUP_WAIT_TICKS" ]; do
    grep -q 'frontend_ready_ms=' "$PERF_LOG" 2>/dev/null && break
    sleep 0.2
    waited=$((waited + 1))
  done

  # perf.log は追記される（Task 13 の record_to）。必ず最後の行を読む。
  local setup_ms ready_ms
  setup_ms="$(grep 'rust_setup_ms=' "$PERF_LOG" 2>/dev/null | tail -n1 | sed 's/.*=//')"
  ready_ms="$(grep 'frontend_ready_ms=' "$PERF_LOG" 2>/dev/null | tail -n1 | sed 's/.*=//')"
  if [ -z "$ready_ms" ]; then
    printf 'FAIL  起動時間          perf.log に frontend_ready_ms がありません (%s)\n' "$PERF_LOG"
    exit_code=1
    return
  fi
  # rust_setup_ms の欠落も FAIL にする。ここが `?` を出して素通りすると、
  # setup クロージャの record("rust_setup_ms") はどこにも観測点を持たない。
  if [ -z "$setup_ms" ]; then
    printf 'FAIL  起動時間          perf.log に rust_setup_ms がありません (%s)\n' "$PERF_LOG"
    exit_code=1
    return
  fi
  printf 'INFO  内訳 rust_setup_ms=%s\n' "$setup_ms"
  verdict "起動時間" "$ready_ms" "$STARTUP_BUDGET_MS" "ms" || exit_code=1
}

cmd_memory() {
  printf '== メモリ ==\n'
  local root
  root="$(app_pid)"
  if [ -z "$root" ]; then
    printf 'FAIL  メモリ            %s が起動していません\n' "$APP_NAME"
    exit_code=1
    return
  fi
  local table measured tree_all non_pty
  table="$(ps_snapshot)"
  if ! measured="$(measured_pids "$root" "$table")"; then
    printf 'FAIL  メモリ            §104.2 の 4 プロセスを特定できません（WebKit ヘルパ 3 本がそろっていない）\n'
    exit_code=1
    return
  fi
  tree_all="$(descendants "$root" "$table")"
  non_pty="$(non_pty_pids "$root" "$table")"
  # shellcheck disable=SC2086
  printf 'INFO  参考値 1: 子孫すべて（PTY 込み） %s MB\n' "$(rss_mb_in "$table" $tree_all)"
  # shellcheck disable=SC2086
  printf 'INFO  参考値 2: 子孫 − PTY 子孫        %s MB\n' "$(rss_mb_in "$table" $non_pty)"
  printf 'INFO  統制対象 PID（§104.2 の 4 プロセス）: %s\n' "$measured"
  # shellcheck disable=SC2086
  verdict "メモリ" "$(rss_mb_in "$table" $measured)" "$MEM_BUDGET_MB" "MB" || exit_code=1
}

cmd_idle_cpu() {
  printf '== アイドル CPU ==\n'
  local root
  root="$(app_pid)"
  if [ -z "$root" ]; then
    printf 'FAIL  アイドルCPU       %s が起動していません\n' "$APP_NAME"
    exit_code=1
    return
  fi
  local table measured
  table="$(ps_snapshot)"
  if ! measured="$(measured_pids "$root" "$table")"; then
    printf 'FAIL  アイドルCPU       §104.2 の 4 プロセスを特定できません（WebKit ヘルパ 3 本がそろっていない）\n'
    exit_code=1
    return
  fi
  printf 'INFO  %s 秒間サンプリングします。この間アプリを操作しないでください\n' "$IDLE_SAMPLES"

  local args="" p
  for p in $measured; do args="$args -pid $p"; done

  # top を 1 回だけ走らせる。最初のブロックは top 起動直後のノイズなので捨てる
  local out
  # shellcheck disable=SC2086
  out="$(top -l $((IDLE_SAMPLES + 1)) -s 1 -stats pid,cpu $args 2>/dev/null | awk '
    /^ *PID/ { block++; if (block > 1) { n++; sample[n] = 0 } next }
    block > 1 && $1 ~ /^[0-9]+$/ { sample[n] += $2 }
    END {
      if (n == 0) { print "NA NA"; exit }
      for (i = 1; i <= n; i++) { t += sample[i]; if (sample[i] > pk) pk = sample[i] }
      printf "%.2f %.2f", t / n, pk
    }')"
  local mean peak
  mean="$(printf '%s' "$out" | awk '{ print $1 }')"
  peak="$(printf '%s' "$out" | awk '{ print $2 }')"
  if [ "$mean" = "NA" ] || [ -z "$mean" ]; then
    printf 'FAIL  アイドルCPU       top からサンプルを取得できませんでした\n'
    exit_code=1
    return
  fi
  verdict "アイドルCPU平均" "$mean" "$IDLE_CPU_MEAN_MAX" "%" || exit_code=1
  verdict "アイドルCPU最大" "$peak" "$IDLE_CPU_PEAK_MAX" "%" || exit_code=1
}

main() {
  case "${1:-all}" in
    startup) cmd_startup ;;
    memory) cmd_memory ;;
    idle-cpu) cmd_idle_cpu ;;
    all)
      cmd_startup
      cmd_memory
      cmd_idle_cpu
      ;;
    *)
      printf 'usage: %s [startup|memory|idle-cpu|all]\n' "$0" >&2
      exit 2
      ;;
  esac
  printf '\n終了コード: %s\n' "$exit_code"
  exit "$exit_code"
}

# KAMUX_LIB_ONLY=1 で source すると関数だけ読み込む（テスト用）
if [ "${KAMUX_LIB_ONLY:-0}" != "1" ]; then
  main "$@"
fi
