#!/usr/bin/env python3
r"""台帳（.claude/AGENT-LESSONS.md）の列数検算の正典。

素の `|`（escape されていない列区切り）だけを数え、表ブロックごとにヘッダ行と本数を
突き合わせる。`awk -F'|'` は escape 済み `\\|` も区切りに数えるため、正常な行を NG と
報告する偽陽性を作る（#58 ほか 5 行が 12 日間「壊れた行」として扱われた実例）。

終了コード: 0 = 全ブロック整合 / 1 = 不整合あり / 2 = 前提不正（ファイルが読めない等）。
使い方: リポジトリのルートから python3 .claude/skills/kamux-promotion/scripts/check-ledger-columns.py
       [対象ファイル ...]（省略時は .claude/AGENT-LESSONS.md）

母数の外（守る対象のうち本検査が見ないもの）:
- 先頭が `|` でない行から始まる表（GFM では正当な書き方）は 1 行も見ない。母数（表ブロック数と行数）を
  出力するので、対象ファイルに表が在るのに 0 ブロックなら空振りを疑うこと。
- code span 内の素の `|` を落とさないのは意図的である（GFM は表セル内では code span の中でも `\|` を
  要求する）。「markdown の機械検査は code span を先に落とす」を当てて本挙動を壊さないこと。
"""
import re
import sys

RAW_PIPE = re.compile(r"(?<!\\)\|")


def check(path: str) -> int:
    try:
        with open(path, encoding="utf-8") as f:
            lines = f.read().split("\n")
    except OSError as e:
        print(f"FATAL: {path}: {e}")
        return 2
    ng = 0
    nblocks = 0
    nrows = 0
    i = 0
    while i < len(lines):
        if lines[i].startswith("|"):
            j = i
            while j < len(lines) and lines[j].startswith("|"):
                j += 1
            nblocks += 1
            nrows += j - i
            head = len(RAW_PIPE.findall(lines[i]))
            for k in range(i, j):
                got = len(RAW_PIPE.findall(lines[k]))
                if got != head:
                    ng += 1
                    print(f"NG {path}:{k + 1} 素の '|' が {got} 本（ブロックのヘッダは {head} 本）")
            i = j
        else:
            i += 1
    blocks = "整合" if ng == 0 else f"不整合 {ng} 行"
    print(f"{path}: {blocks}（母数: 表ブロック {nblocks} / 行 {nrows}）")
    return 0 if ng == 0 else 1


def main() -> int:
    targets = sys.argv[1:] or [".claude/AGENT-LESSONS.md"]
    worst = 0
    for t in targets:
        rc = check(t)
        worst = max(worst, rc)
    return worst


if __name__ == "__main__":
    sys.exit(main())
