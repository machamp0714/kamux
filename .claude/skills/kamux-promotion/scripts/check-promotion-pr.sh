#!/usr/bin/env bash
# 昇格 PR の自己点検。SKILL.md「マージ前の点検」が唯一の呼び出し元であり、各検査の「なぜこの形か」は
# 同節の小見出しが持つ。ここへ理由を書き写さない（二重の正典になる）。
#
#   scripts/check-promotion-pr.sh --pr <n> [--approved-sha <sha>] [--ledger <path>] [--no-promotion]
#
# 終了コード: 0 = 全検査合格 / 1 = 1 つ以上不合格 / 2 = 引数か前提が不正で測定できていない
# 🔴 2 と 1 を混ぜないこと。「測れていない」と「不合格」を同じ出力にすると空振りが検出できない。

set -uo pipefail

PR="" APPROVED_SHA="" LEDGER=".claude/AGENT-LESSONS.md" NO_PROMOTION=0
SKILL_REL=".claude/skills/kamux-promotion/SKILL.md"
CANON_PHRASE='同じ穴が 2 回以上'

die() { printf 'FATAL: %s\n' "$1" >&2; exit 2; }

# 🔴 値を取るオプションでは、値の存在を先に確かめてから shift 2 する。
# 値が無いまま `shift 2` を打つとシフトが失敗し、$# が減らないので while が無限に回る（実際に踏んだ）。
while [ $# -gt 0 ]; do
  case "$1" in
    --pr)           [ $# -ge 2 ] || die "--pr に値が必要である。"          ; PR="$2";           shift 2 ;;
    --approved-sha) [ $# -ge 2 ] || die "--approved-sha に値が必要である。"; APPROVED_SHA="$2"; shift 2 ;;
    --ledger)       [ $# -ge 2 ] || die "--ledger に値が必要である。"      ; LEDGER="$2";       shift 2 ;;
    --no-promotion) NO_PROMOTION=1; shift ;;
    *) die "未知の引数 '$1'。使い方: --pr <n> [--approved-sha <sha>] [--ledger <path>] [--no-promotion]" ;;
  esac
done

[ -n "$PR" ] || die "--pr が必要である。昇格 PR の番号を渡すこと。"
[ -f "$LEDGER" ] || die "台帳 '$LEDGER' が無い。リポジトリのルートで実行しているか確認すること（--ledger で明示もできる）。"
[ -f "$SKILL_REL" ] || die "'$SKILL_REL' が無い。リポジトリのルートで実行すること。"
command -v gh >/dev/null 2>&1 || die "gh が無い。PR 本文を読めないので検査 2 から 4 が測定不能である。"

BODY="$(gh pr view "$PR" --json body -q .body 2>/dev/null)" \
  || die "gh pr view $PR に失敗した。PR 番号とネットワーク／認証を確認すること。"
[ -n "$BODY" ] || die "PR #$PR の本文が空である。最終コミット後に本文を取り直すこと。"

FAIL=0
ng() { printf 'NG  [%s] %s\n' "$1" "$2" >&2; FAIL=1; }
ok() { printf 'OK  [%s] %s\n' "$1" "$2"; }

# --- 検査 1: 閾値・手続きの正典が本スキル 1 ファイルだけであること -------------------
# --exclude-dir=worktrees が要る。.claude/worktrees/ には他レーンの複製が在る。
# 本スクリプト自身が CANON_PHRASE を変数として持つので、自分だけを外す。
# 🔴 --include='*.md' で絞ってはならない。md 以外に正典フレーズが紛れ込んでも見えなくなる。
CANON="$(grep -rl "$CANON_PHRASE" .claude --exclude-dir=worktrees --exclude="$(basename "$0")" 2>/dev/null | sort)"
CANON_N="$(printf '%s' "$CANON" | grep -c .)"
if [ "$CANON_N" -eq 0 ]; then
  ng 1 "'$CANON_PHRASE' がどこにも無い。母数 0 は「違反なし」ではなく「測れていない」である。SKILL_REL か CANON_PHRASE が実物とずれた"
elif [ "$CANON_N" -eq 1 ] && [ "$CANON" = "$SKILL_REL" ]; then
  ok 1 "閾値の正典は $SKILL_REL の 1 ファイルだけである"
else
  ng 1 "閾値の正典が $CANON_N ファイルに在る（二重の正典）: $(printf '%s' "$CANON" | tr '\n' ' ')"
fi

# --- 検査 2: ユーザーの承認 ----------------------------------------------------
if printf '%s' "$BODY" | grep -q '^ユーザー承認:'; then
  ok 2 "PR 本文に 'ユーザー承認:' の行が在る"
  if [ -n "$APPROVED_SHA" ]; then
    if ! git cat-file -e "${APPROVED_SHA}^{commit}" 2>/dev/null; then
      die "承認 sha '$APPROVED_SHA' がこのリポジトリに存在しない。取り違えていないか確認すること。"
    fi
    D="$(git diff --no-ext-diff --stat "$APPROVED_SHA"..HEAD)"
    if [ -z "$D" ]; then ok 2 "承認 sha から HEAD までの差分は空である"
    else ng 2 "承認 sha 以降にコミットが積まれている。「ユーザーの承認」の手順 2 から承認を取り直すこと:"$'\n'"$D"; fi
  else
    printf 'SKIP[2] --approved-sha 未指定のため差分検査を実行していない。マージ前には必ず渡すこと。\n' >&2
  fi
else
  ng 2 "PR 本文に 'ユーザー承認: <日時> / 承認時の sha: <sha>' の行が無い。ユーザー承認のゲートを通っていない"
fi

# --- 検査 3: 必須 3 項目が PR 本文に在ること ------------------------------------
# 🔴 1 本の grep -cE にまとめない。-c は行数を返すので 1 項目が 0 行でも合計は非 0 になる。
for K in 'なぜ' '観測点' '置いたロール' '置かなかったロール'; do
  N="$(printf '%s' "$BODY" | grep -c "$K")"
  if [ "$N" -gt 0 ]; then ok 3 "PR 本文に「${K}」が $N 行"
  else ng 3 "PR 本文に「${K}」が 1 行も無い（必須項目）"; fi
done

# --- 検査 4: 閉じる台帳行の照合 -----------------------------------------------------
LEDGER_ROWS="$(grep -cE '^\| [0-9]+ \|' "$LEDGER")"
LEDGER_OPEN="$(grep -cE '^\| [0-9]+ \|.*\| — \|$' "$LEDGER")"
printf 'INFO[4] 台帳の母数=%s 未昇格=%s\n' "$LEDGER_ROWS" "$LEDGER_OPEN"
[ "$LEDGER_ROWS" -gt 0 ] || die "台帳の事象行が 0 件。行の書式か --ledger の指定がずれている（母数 0 では検査 4 は意味を持たない）。"

if [ "$NO_PROMOTION" -eq 1 ]; then
  # 🔴 フラグと本文の矛盾を先に見る。宣言が在るのに --no-promotion を付けると、
  # 実在する閉じ忘れを無条件に素通りさせて PASS を返す（レビュー r5 が実測）。
  if printf '%s' "$BODY" | grep -q '^閉じる台帳行:'; then
    ng 4 "--no-promotion を指定したが、PR 本文に '閉じる台帳行:' の宣言が在る。どちらかが誤り。昇格させる行があるならフラグを外し、無いなら宣言行を消すこと"
  else
    printf 'SKIP[4] --no-promotion 指定。宣言行も無いので照合しない。\n' >&2
  fi
else
  DECL="$(printf '%s' "$BODY" | sed -n 's/^閉じる台帳行: *//p' | tr ' ' '\n')"
  A="$(printf '%s' "$DECL" | grep -c .)"                 # ③a 全 token 数
  B="$(printf '%s' "$DECL" | grep -cE '^[0-9]+$')"       # ③b 数値 token 数
  WANT="$(printf '%s' "$DECL" | grep -E '^[0-9]+$' | sort -u)"
  C="$(printf '%s' "$WANT" | grep -c .)"                 # ③c 重複除去後
  printf 'INFO[4] 宣言 token=%s 数値 token=%s 重複除去後=%s\n' "$A" "$B" "$C"

  if [ "$A" -eq 0 ]; then
    ng 4 "PR 本文に '閉じる台帳行: <ID...>' の行が無い（「昇格 PR の進め方」の義務）。宣言が無いと照合は自明に合格するので不合格として扱う。昇格 0 件の便なら --no-promotion を渡すこと"
  else
    [ "$A" -eq "$B" ] || ng 4 "宣言に数値でない token が $((A - B)) 個ある（カンマ区切り等）。半角空白区切りで書き直すこと。落ちた token は照合されず、閉じ忘れが素通りする"
    [ "$B" -eq "$C" ] || ng 4 "同じ台帳行 ID を $((B - C)) 個二重に宣言している。重複を除いて書き直すこと"

    MISSING="" NOTCLOSED=""
    while IFS= read -r i; do
      [ -n "$i" ] || continue
      # ③d 実在検査。落とすと、実在しない ID が下の行で「閉じた」側へ吸収される
      grep -qE "^\| $i \|" "$LEDGER" || { MISSING="$MISSING $i"; continue; }
      grep -qE "^\| $i \|.*\| — \|$" "$LEDGER" && NOTCLOSED="$NOTCLOSED $i"
    done <<< "$WANT"

    [ -z "$MISSING" ]   || ng 4 "宣言した ID が台帳に実在しない:${MISSING}（実在しない ID は「閉じた」側へ吸収され、警告なく消える）"
    [ -z "$NOTCLOSED" ] || ng 4 "宣言したのに \`昇格\` 列が \`—\` のまま残っている:${NOTCLOSED}（閉じ忘れ）"
    [ -n "$MISSING$NOTCLOSED" ] || ok 4 "宣言した $C 行はすべて実在し、すべて閉じている"
  fi
fi

# --- 判定 ---------------------------------------------------------------------------
if [ "$FAIL" -eq 0 ]; then
  printf '\nPASS: 昇格 PR #%s は 全検査に合格した。\n' "$PR"; exit 0
else
  printf '\nFAIL: 上の NG を解消してから再実行すること。SKIP の行は「合格」ではなく「測っていない」である。\n' >&2; exit 1
fi
